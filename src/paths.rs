use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;

use crate::cli::ArchiveType;
use crate::cli::FormatFileExt;


pub async fn validate_and_unglob(mut paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
	let unexisting = paths.extract_if(|p| !p.try_exists().ok().unwrap_or(false));
	let mut resolved = Vec::new();
	for query in unexisting {
		let current = resolved.len();
		resolved.extend(unglob(query.to_string_lossy()).await?);

		if current == resolved.len() {
			warn!(
			      "Path or glob pattern '{}' is wrong and will be ignored",
			      query.display()
			);
		}
	}
	paths.append(&mut resolved);

	// Need dedup but not sort because we want to keep the order, user's preferred order...
	// Or not? Ok, just sort & dedup:
	paths.sort();
	paths.dedup();

	Ok(paths)
}

pub async fn unglob<S: AsRef<str>>(pattern: S)
                                   -> Result<impl Iterator<Item = PathBuf>, Box<dyn std::error::Error>> {
	use glob::glob;
	Ok(glob(pattern.as_ref())?.filter_map(|res| res.map_err(|err| error!("{err}")).ok()))
}


/// Prepare source path to concat with dst path.
fn sanitize_path(path: &Path) -> PathBuf {
	if path.is_absolute() {
		path.components().skip(1).collect()
	} else {
		path.components()
		    .filter(|c| !matches!(c, std::path::Component::ParentDir))
		    .collect()
	}
}


pub fn output_archive_path(source: impl AsRef<Path>, outdir: impl AsRef<Path>, archive: ArchiveType) -> PathBuf {
	let source = source.as_ref();
	let subpath = if source.is_absolute() {
		source.file_name()
		      .map(Path::new)
		      .or(source.parent())
		      .expect("invalid path")
		      .with_extension(archive.ext())
	} else {
		sanitize_path(source).with_extension(archive.ext())
	};

	let output = outdir.as_ref().join(subpath);
	output
}


pub type StringEntry = Entry<usize, String>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Entry<I, S: AsRef<OsStr>> {
	pub index: I,
	pub uri: S,
}

impl<I, S: AsRef<OsStr>> AsRef<OsStr> for Entry<I, S> {
	fn as_ref(&self) -> &OsStr { self.uri.as_ref() }
}

impl<I, S: AsRef<OsStr>> From<(I, S)> for Entry<I, S> {
	fn from((index, uri): (I, S)) -> Self { Self { index, uri } }
}


pub fn filter_entries<S: AsRef<OsStr>>(entries: impl Iterator<Item = S> + Send)
                                       -> impl Iterator<Item = S> + Send {
	entries.filter(|entry| {
		       let s = entry.as_ref().to_string_lossy();
		       let uri = Path::new(&entry);
		       let skip = s.ends_with("/") ||
		                  uri.ends_with("Thumbs.db") ||
		                  uri.file_name() == Some(&OsStr::new(".DS_Store")) ||
		                  uri.iter()
		                     .filter(|item| *item == OsStr::new("__MACOSX"))
		                     .filter(|item| *item == OsStr::new(".DS_Store"))
		                     .filter(|item| item.len() > 1 && item.to_string_lossy().starts_with("."))
		                     .next()
		                     .is_some();

		       if skip {
			       trace!("outfiltered inner file: {s}");
		       }
		       !skip
	       })
}


/// Try to find root dir in one pass.
/// Algorithm is stupidly simple:
/// - find first component of path without ext => this is potential root dir
/// - find the same second time => this is exactly root dir.
/// - filter out paths that are exactly equal root dir.
/// That's doesn't work properly if root dir is first item of given iterator.
pub fn remove_root_entry<S: AsRef<OsStr>>(entries: impl Iterator<Item = S> + Send)
                                          -> impl Iterator<Item = S> + Send {
	let mut root: Option<std::ffi::OsString> = None;
	let mut possible: Option<std::ffi::OsString> = None;

	entries.filter(move |entry| {
		       if let Some(root) = root.as_deref() {
			       return entry.as_ref() != root;
		       };

		       // inspect the entry
		       let mut parts = Path::new(&entry).components();

		       // get first:
		       let first = if let Some(first) = parts.next() {
			       first.as_os_str()
		       } else {
			       // empty name, almost impossible case:
			       return false;
		       };


		       if Path::new(first).extension().is_none() {
			       // root = possible:
			       if possible.as_deref() == Some(first) {
				       root = possible.take();
				       debug!("found root: {:?}", root.as_ref().unwrap());
			       }
			       // this is possible:
			       else {
				       possible = Some(first.to_owned());
				       debug!("found possible: {first:?}");
			       }
		       }
		       // else ignore

		       // and finally check with newly potentially found root:
		       Some(entry.as_ref()) != root.as_deref()
	       })
}
