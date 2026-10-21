pub mod image;
pub mod stl;

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("file contains no triangles")]
    Empty,
    #[error("malformed file: {0}")]
    Malformed(String),
    #[error("image: {0}")]
    Image(#[from] ::image::ImageError),
}
