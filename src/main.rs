#![feature(drain_filter)]
#![feature(never_type)]

#[macro_use]
extern crate log;
extern crate tokio;
use std::fmt::Debug;
use std::sync::Arc;
use std::path::{Path, PathBuf};

use tokio::fs::try_exists;
use tokio::sync::RwLock;
use futures::TryFutureExt;
use futures::{stream, StreamExt};
use tokio_util::compat::TokioAsyncWriteCompatExt;
use archive_reader::Archive;
use async_zip::ZipEntryBuilder;
use async_zip::tokio::write::ZipFileWriter;
use indicatif::MultiProgress;
use image::ImageOutputFormat;
use image::ImageEncoder;


mod cli;
mod logger;
mod error;
mod paths;

use error::Error;
use cli::Config;
use cli::FormatFileExt;


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let mut args = cli::parse();

	let multibar = MultiProgress::new();
	multibar.set_move_cursor(true);
	let bar_completed = cli::main_progress_bar(&multibar)?;


	logger::init(args.verbose, Some(multibar.clone()))?;
	trace!("input args: {:#?}", args);


	debug!("preparing input paths");
	let sources = paths::validate_and_unglob(args.input).await?;

	bar_completed.set_length(sources.len() as _);
	bar_completed.set_position(0 as _);

	debug!("preparing output path");
	let outdir = if let Some(output) = args.output {
		tokio::fs::create_dir_all(&output).await?;
		output
	} else {
		std::env::current_dir()? // XXX: potential inplace race & corruption!
	};


	let concurrency = args.jobs_fs;
	args.config.jobs /= concurrency;

	let create_inout_task = |path: PathBuf| {
		let outdir = outdir.clone();
		let config = args.config.clone();
		let multibar = multibar.clone();

		let set_initial_progress = |inout: ProcessInOut| async move { Ok(inout) };

		// TODO: remove this scope-wrapper:
		async move {
			open_inout(path, outdir, &config).and_then(set_initial_progress)
			                                 .and_then(|inout| convert_all(inout, &config, Some(multibar)))
			                                 .and_then(|res| {
				                                 async move {
					                                 let sp = res.src.display();
					                                 let src = tokio::fs::metadata(&res.src).await?.len();
					                                 let dst = res.dst.len();
					                                 let p = (dst as f64 / src as f64) * 100.0;
					                                 // TODO: this should be `info`:
					                                 debug!("Archived: {sp}, new size: {dst}b vs. {src}b ≈ {p:.2}%",);
					                                 Ok(res)
				                                 }
			                                 })
			                                 .await
		}
	};

	let notify = |res: Result<ConversionResult, _>| {
		let bar_completed_ref = &bar_completed;
		async move {
			match res {
				Ok(res) => info!("Finished: {}", res.src.display()),
				Err(err) => error!("{err}"),
			}
			bar_completed_ref.inc(1);
		}
	};


	stream::iter(sources.into_iter()).map(create_inout_task)
	                                 .buffer_unordered(concurrency)
	                                 .for_each(notify)
	                                 .await;

	info!("Complete 🎉");
	multibar.clear()?;
	log::logger().flush();
	Ok(())
}


enum ArchiveWriter {
	Zip(ZipFileWriter<tokio::fs::File>),
	Sz(sevenz_rust::SevenZWriter<std::fs::File>),
}

impl ArchiveWriter {
	async fn open_file(path: impl AsRef<Path>, force: bool) -> Result<tokio::fs::File, Error> {
		let path = path.as_ref();
		debug!("opening output: '{}'", path.display());
		let out_exists = try_exists(&path).await?;

		if out_exists && !force {
			return Err(std::io::Error::new(
				std::io::ErrorKind::AlreadyExists,
				format!("Output file already exists {}", path.display()),
			).into());
		}

		if let Some(parent) = path.parent() {
			tokio::fs::create_dir_all(parent).await?;
		}

		let output_file = tokio::fs::OpenOptions::new().write(true)
		                                               .create_new(!out_exists)
		                                               .truncate(force)
		                                               .open(&path)
		                                               .await?;
		Ok(output_file)
	}

	pub async fn open_zip(path: impl AsRef<Path>, force: bool) -> Result<Self, Error> {
		let output_file = Self::open_file(path, force).await?;
		let writer = ZipFileWriter::new(output_file.compat_write());
		Ok(Self::Zip(writer))
	}

	pub async fn open_7z(path: impl AsRef<Path>, force: bool) -> Result<Self, Error> {
		use sevenz_rust::*;

		let output_file = Self::open_file(path, force).await?;
		let mut writer = SevenZWriter::new(output_file.into_std().await)?;
		writer.set_content_methods(vec![SevenZMethodConfiguration::new(SevenZMethod::LZMA2).with_options(
			MethodOptions::LZMA2(lzma::LZMA2Options::with_preset(9)),
		)]);

		Ok(Self::Sz(writer))
	}


	pub async fn write_all(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
		debug!("writing '{name}' to output archive");
		match self {
			Self::Zip(writer) => {
				let compression = async_zip::Compression::Deflate;
				let builder = ZipEntryBuilder::new(name.into(), compression).deflate_option(async_zip::DeflateOption::Maximum);
				writer.write_entry_whole(builder, data).await?;
			},

			Self::Sz(writer) => {
				use sevenz_rust::*;
				let mut entry = SevenZArchiveEntry::default();
				entry.name = name.to_owned();
				writer.push_archive_entry(entry, Some(data))?;
			},
		}
		Ok(())
	}


	pub async fn close(self) -> Result<std::fs::Metadata, Error> {
		let meta = match self {
			Self::Zip(writer) => {
				let f = writer.close().await?.into_inner();
				let meta = f.metadata().await?;
				f.sync_data().await?;
				meta
			},
			Self::Sz(writer) => {
				let f = writer.finish()?;
				let meta = f.metadata()?;
				f.sync_data()?;
				meta
			},
		};
		Ok(meta)
	}
}


struct ConversionResult {
	src: PathBuf,
	dst: std::fs::Metadata,
}

async fn convert_all(mut inout: ProcessInOut, cfg: &Config, multibar: Option<MultiProgress>) -> Result<ConversionResult, Error> {
	let jobs = cfg.jobs;
	trace!("jobs per archive: {jobs}");
	let source = inout.reader.path().to_owned();
	let writer = Arc::new(RwLock::new(&mut inout.writer));

	let bar = multibar.map(|mb| {
		                  let len = inout.total_entries;
		                  let pos = len - inout.entries.len();
		                  let text = inout.reader.path().file_name().unwrap().to_string_lossy().to_string();
		                  cli::sub_progress_bar(&mb, len, pos, text)
	                  });

	let convert_entry = |entry: paths::StringEntry| {
		let source = &source;
		let name = entry.uri.to_owned();
		let reader = inout.reader.clone();
		let bar = &bar;

		// Read entries, then convert them, then write to resulting archive
		async move {
			debug!("reading '{name}'");
			let mut buffer = Vec::new();
			let ar_size = reader.read_file(&name, &mut buffer)?;
			let raw_size = buffer.len();
			let name = name.to_owned();

			// TODO: mb. use name.filename instead of name

			if ar_size == 0 {
				Err(format!("no data in '{}:{name}'", source.display()).into())
			} else {
				debug!("transcoding '{name}'");
				let (name, data) = tokio::spawn(transcode(cfg.clone(), buffer, name.clone())).await??;
				// TODO: this log should be `info`:
				debug!(
				       "Encoded: {name}, new size: {}b vs. {}b ≈ {:.2}%",
				       data.len(),
				       raw_size,
				       (data.len() as f64 / raw_size as f64) * 100.0
				);
				bar.as_ref().map(|bar| bar.inc(1));
				Ok::<_, Error>((data, name))
			}
		}.and_then(|(data, name)| {
			let writer = writer.clone();
			async move {
				writer.write().await.write_all(&name, &data[..]).await?;
				Ok(name)
			}
		})
	};

	stream::iter(inout.entries.into_iter()).map(convert_entry)
	                                       .buffer_unordered(jobs)
	                                       .for_each(|res| {
		                                       async move {
			                                       match res {
				                                       Ok(name) => info!("Finished: {name}"),
			                                          Err(err) => error!("{err}"),
			                                       }
		                                       }
	                                       })
	                                       .await;
	inout.writer.close().await.map(|dst| ConversionResult { src: source, dst })
}


struct ProcessInOut {
	reader: Arc<Archive>,
	/// Inner files remains to process, already resolved and filtered
	entries: Vec<paths::StringEntry>,
	/// total number of entries before any filtering
	total_entries: usize,

	writer: ArchiveWriter,
}

async fn open_inout(source: impl AsRef<Path>, outdir: impl AsRef<Path>, cfg: &Config) -> Result<ProcessInOut, Error> {
	use cli::ArchiveType::*;
	let (reader, entries, total) = archive_reader(&source).await?;
	let output = paths::output_archive_path(&source, &outdir, cfg.archive);
	let writer = match cfg.archive {
		Cbz | Zip => ArchiveWriter::open_zip(output.as_path(), cfg.force).await?,
		Cb7 | SevenZip => ArchiveWriter::open_7z(output.as_path(), cfg.force).await?,
	};
	Ok(ProcessInOut { reader: Arc::new(reader),
	                  entries,
	                  writer,
	                  total_entries: total })
}


async fn archive_reader(path: impl AsRef<Path>) -> Result<(Archive, Vec<paths::StringEntry>, usize), Error> {
	debug!("opening input: '{}'", path.as_ref().display());
	let mut archive = Archive::open(&path.as_ref());
	archive.block_size(1024 * 1024);

	trace!("filtering inner files");
	let mut total = 0_usize;
	let names = paths::filter_entries(archive.list_file_names()?
	                                         .enumerate()
	                                         .filter_map(|(i, name)| {
		                                         total += 1;
		                                         name.ok().map(|s| (i, s))
	                                         })
	                                         .map(paths::Entry::from));
	let names: Vec<_> = paths::remove_root_entry(names).collect();
	debug!("total: {total}, outfiltered: {}", total - names.len());
	Ok((archive, names, total))
}


async fn transcode<S: AsRef<str> + Debug>(cfg: Config, data: Vec<u8>, name: S) -> Result<(String, Vec<u8>), image::ImageError> {
	let cfg = cfg.clone();
	let uri = Path::new(name.as_ref());
	let filename = uri.file_name().expect("filename").to_owned();
	let format = uri.extension()
	                .and_then(|ext| ext.to_str())
	                .map(image::ImageFormat::from_extension)
	                .flatten();

	if format.is_none() {
		if let Some(ext) = uri.extension() {
			match ext.to_string_lossy().as_ref().to_lowercase().as_str() {
				"txt" | "md" | "xml" | "html" | "svg" | "info" | "json" | "yml" | "yaml" => {
					debug!("'{}' Seems to text, so just copying as-is.", uri.display());
					return Ok((name.as_ref().to_string(), data));
				},
				_ => {},
			}
		}
	}

	let out_format = match &cfg.format {
		ImageOutputFormat::Jpeg(_) => ImageOutputFormat::Jpeg(cfg.quality.clamp(0, 100)),
		format => format.to_owned(),
	};

	if Some(&out_format) == format.map(ImageOutputFormat::from).as_ref() {
		warn!("SKIP with reason: same format: {out_format:?}");
		return Ok((filename.to_string_lossy().to_string(), data));
	}

	if matches!(format, Some(image::ImageFormat::WebP) | Some(image::ImageFormat::Avif)) {
		warn!("SKIP with reason: src is already good format: {:?}", format.as_ref().unwrap());
		return Ok((filename.to_string_lossy().to_string(), data));
	}


	let image = if let Some(format) = format {
		image::load_from_memory_with_format(&data, format)
	} else {
		image::load_from_memory(&data)
	};


	if let Ok(image) = image {
		trace!(
		       "original image: {}, len: {} ({format:?}, {:?})",
		       uri.display(),
		       data.len(),
		       image.color()
		);

		let mut output: Vec<u8> = Vec::new();

		match &cfg.format {
			ImageOutputFormat::Avif => {
				use image::codecs::avif::{AvifEncoder, ColorSpace};
				AvifEncoder::new_with_speed_quality(&mut output, cfg.speed, cfg.quality).with_colorspace(ColorSpace::Bt709)
				                                                                        .write_image(
				                                                                                     image.as_bytes(),
				                                                                                     image.width(),
				                                                                                     image.height(),
				                                                                                     image.color(),
				)?;
			},

			ImageOutputFormat::WebP => {
				use image::codecs::webp::{WebPEncoder, WebPQuality};
				let quality = if cfg.lossless {
					WebPQuality::lossless()
				} else {
					WebPQuality::lossy(cfg.quality)
				};
				WebPEncoder::new_with_quality(&mut output, quality).write_image(
				                                                                image.as_bytes(),
				                                                                image.width(),
				                                                                image.height(),
				                                                                image.color(),
				)?;
			},

			ImageOutputFormat::Png => {
				use image::codecs::png::{PngEncoder, CompressionType, FilterType};
				PngEncoder::new_with_quality(&mut output, CompressionType::Best, FilterType::Adaptive).write_image(
				                                                                                                   image.as_bytes(),
				                                                                                                   image.width(),
				                                                                                                   image.height(),
				                                                                                                   image.color(),
				)?;
			},
			format => {
				use std::io::Cursor;
				image.write_to(&mut Cursor::new(&mut output), format.to_owned())?
			},
		}


		let filename = Path::new(&filename).with_extension(cfg.format.ext()).display().to_string();
		trace!("transcoded image: {filename}, len: {} ({:?})", data.len(), cfg.format);


		Ok((filename, output))
	} else {
		warn!("Unable to decode as image: {}, so just copying as-is.", uri.display());
		Ok((name.as_ref().to_string(), data))
	}
}
