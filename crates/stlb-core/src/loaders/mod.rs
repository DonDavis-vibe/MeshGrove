pub mod image;
pub mod pdf;
pub mod stl;
pub mod threemf;

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

/// Geometry of any supported model file.
pub fn load_mesh(kind: crate::Kind, bytes: &[u8]) -> Result<crate::Mesh, LoadError> {
    match kind {
        crate::Kind::Stl => stl::parse(bytes),
        crate::Kind::ThreeMf => Ok(threemf::parse(bytes)?.mesh),
        other => Err(LoadError::Malformed(format!("{other:?} is not a mesh format"))),
    }
}
