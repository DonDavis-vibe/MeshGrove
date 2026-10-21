pub mod gltf;
pub mod image;
pub mod obj;
pub mod paint;
pub mod pdf;
pub mod ply;
pub mod step;
pub mod stl;
pub mod texture;
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

/// Reads a file a model refers to by its path relative to the model (OBJ materials and
/// textures, glTF buffers and images); `None` if there is no such file.
pub type Sibling<'a> = &'a dyn Fn(&str) -> Option<Vec<u8>>;

/// Geometry of any supported model file that needs no files next to it (STEP goes through
/// [`step::load`]).
pub fn load_mesh(kind: crate::Kind, bytes: &[u8]) -> Result<crate::Mesh, LoadError> {
    load_mesh_with(kind, bytes, &|_| None)
}

/// Like [`load_mesh`], with the files next to the model for materials, textures and buffers.
pub fn load_mesh_with(kind: crate::Kind, bytes: &[u8], sibling: Sibling) -> Result<crate::Mesh, LoadError> {
    match kind {
        crate::Kind::Stl => stl::parse(bytes),
        crate::Kind::ThreeMf => Ok(threemf::parse(bytes)?.mesh),
        crate::Kind::Obj => obj::parse(bytes, sibling),
        crate::Kind::Ply => ply::parse(bytes),
        crate::Kind::Gltf => gltf::parse(bytes, sibling),
        other => Err(LoadError::Malformed(format!("{other:?} is not a mesh format"))),
    }
}
