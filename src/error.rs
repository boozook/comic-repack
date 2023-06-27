extern crate thiserror;


#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("IO: {0}")]
	Io(#[from] std::io::Error),

	#[error("Ar-read: {0}")]
	ArRead(#[from] archive_reader::Error),

	#[error("Zip-write: {0}")]
	ZipWrite(#[from] async_zip::error::ZipError),

	#[error("7Zip-write: {0}")]
	SZipWrite(#[from] sevenz_rust::Error),

	#[error("Encoding: {0}")]
	ImageError(#[from] image::ImageError),

	#[error("Async task join: {0}")]
	AsyncTaskError(#[from] tokio::task::JoinError),

	#[error("{0}")]
	Other(String),
}


impl From<String> for Error {
	fn from(value: String) -> Self { Self::Other(value) }
}
