extern crate clap;
use clap::{Parser, ValueEnum};
use indicatif::{MultiProgress, ProgressStyle, ProgressBar};
use std::{path::PathBuf, borrow::Cow, sync::Arc};


#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
	#[arg(short, long, action = clap::ArgAction::Count, default_value_t = 0)]
	pub verbose: u8,

	#[clap(flatten)]
	pub config: Config,

	/// Input files.
	/// .
	#[arg(last = false, value_name = "FILES")]
	pub input: Vec<PathBuf>,

	/// How many pairs or input-output files will in parallel processing.
	#[arg(short = 'p', long, value_name = "JOBS", default_value_t = 1)]
	pub jobs_fs: usize,

	/// Output directory. Defaults to the current working directory,
	/// so changing input files inplace can be possible and cause a problem. TODO: fix it!
	/// Otherwise, the output path of each produced file will be relative to this directory.
	/// .
	#[arg(last = true, value_name = "OUT DIR")]
	pub output: Option<PathBuf>,
}


fn parse_image_output_format(s: &str) -> Result<image::ImageOutputFormat, String> {
	use image::ImageOutputFormat::{self, *};
	match s.to_lowercase().as_str() {
		"avif" => Ok(Avif),
		"webp" => Ok(WebP),
		"png" => Ok(Png),
		"jpg" | "jpeg" => Ok(Jpeg(100)),
		"gif" => Ok(Gif),
		"bmp" => Ok(Bmp),
		"tga" => Ok(Tga),
		"qoi" => Ok(Qoi),
		"tiff" => Ok(Tiff),
		other => {
			if let Some(format) =
				image::ImageFormat::from_extension(other).map(ImageOutputFormat::from)
				                                         .filter(|f| !matches!(f, ImageOutputFormat::Unsupported(_)))
			{
				Ok(format)
			} else {
				Err(format!("Unsupported image format: {other}"))
			}
		},
	}
}


#[derive(clap::Args, Debug, Clone)]
pub struct Config {
	/// Output image format.
	/// Supported formats: https://docs.rs/image/0.24.6/image/codecs/index.html#supported-formats
	#[arg(short, long, default_value = "avif")]
	#[arg(value_parser = parse_image_output_format)]
	pub format: image::ImageOutputFormat,

	#[arg(short, long, default_value_t = 100)]
	#[arg(value_parser = clap::value_parser!(u8).range(1..=100))]
	pub quality: u8,

	/// Only for webp.
	#[arg(short, long, default_value_t = false)]
	pub lossless: bool,

	/// Used for AVIF encoding, in range 1...10.
	#[arg(short, long, default_value_t = 3)]
	#[arg(value_parser = clap::value_parser!(u8).range(1..=10))]
	pub speed: u8,

	/// Number of parallel threads to use. Defaults to num of physical CPUs - 1.
	#[arg(short, long, default_value_t = (num_cpus::get_physical() - 1).max(1))]
	pub jobs: usize,

	#[arg(short, long, value_name = "TYPE", default_value_t = ArchiveType::Cbz)]
	pub archive: ArchiveType,

	/// Allow overwrite of existing files.
	/// .
	#[arg(long, default_value_t = false)]
	pub force: bool,
}


#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum ArchiveType {
	Cbz,
	Zip,
	Cb7,
	#[value(name = "7z", alias("7z"))]
	SevenZip,
}

impl ToString for ArchiveType {
	fn to_string(&self) -> String { self.ext().to_string() }
}


pub trait FormatFileExt {
	fn ext(&self) -> &str;
}


impl FormatFileExt for image::ImageOutputFormat {
	fn ext(&self) -> &str {
		use image::ImageOutputFormat::*;
		match self {
			Avif => "avif",
			WebP => "webp",
			Jpeg(_) => "jpeg",
			Png => "png",
			Pnm(_) => "pnm",
			Gif => "gif",
			Ico => "ico",
			Bmp => "bmp",
			Tga => "tga",
			Qoi => "qoi",
			Tiff => "tiff",
			Farbfeld => "farbfeld",
			OpenExr => "openexr",
			Unsupported(any) => any.as_str(),
			_ => "",
		}
	}
}


impl FormatFileExt for ArchiveType {
	fn ext(&self) -> &str {
		match self {
			Self::Zip => "zip",
			Self::Cbz => "cbz",
			Self::Cb7 => "cb7",
			Self::SevenZip => "7z",
		}
	}
}


pub fn parse() -> Args { Args::parse() }


// --- progress ---

pub fn sub_progress_bar(multibar: &MultiProgress,
                        len: usize,
                        pos: usize,
                        msg: impl Into<Cow<'static, str>>)
                        -> ProgressBar {
	let template = format!("{{prefix:.bold}} [{{pos:>3}}/{{len:3}}] {{msg:<}} {{wide_bar:.green/.white.dim}} [{{elapsed}}] ({{eta}})");
	let style = ProgressStyle::default_bar().template(&template)
	                                        .unwrap()
	                                        .progress_chars("==-");
	let bar = multibar.add(ProgressBar::new(len as _).with_style(style).with_tab_width(2));
	bar.set_position(pos as _);
	bar.set_message(msg);
	bar
}


pub fn main_progress_bar(multibar: &MultiProgress) -> Result<Arc<ProgressBar>, Box<dyn std::error::Error>> {
	let template = "{prefix:.bold.dim} [{pos}/{len}] {wide_bar:.cyan/.white.dim} {msg} [{elapsed}] ({eta})";
	let style = ProgressStyle::default_bar().template(template)?
	                                        .progress_chars("==-");
	let bar = Arc::new(multibar.add(ProgressBar::new(0).with_style(style).with_tab_width(2)));
	bar.set_prefix("files:");
	Ok(bar)
}
