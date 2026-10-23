pub mod gcode;
pub mod gltf;
pub mod image;
pub mod obj;
pub mod paint;
pub mod pdf;
pub mod ply;
pub mod step;
pub mod stl;
pub mod text;
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
    let mesh = match kind {
        crate::Kind::Stl => stl::parse(bytes),
        crate::Kind::ThreeMf => Ok(threemf::parse(bytes)?.mesh),
        crate::Kind::Obj => obj::parse(bytes, sibling),
        crate::Kind::Ply => ply::parse(bytes),
        crate::Kind::Gltf => gltf::parse(bytes, sibling),
        other => Err(LoadError::Malformed(format!("{other:?} is not a mesh format"))),
    }?;
    Ok(in_file_units(kind, mesh))
}

/// STL, OBJ and PLY carry no unit: they are read in the unit of the settings
/// ([`crate::config::model_units`]) and given in millimetres from here on. Other formats say
/// what they use (3MF, STEP) or have their own setting (glTF).
pub fn in_file_units(kind: crate::Kind, mesh: crate::Mesh) -> crate::Mesh {
    in_units(kind, mesh, crate::config::model_units())
}

fn in_units(kind: crate::Kind, mut mesh: crate::Mesh, units: crate::config::ModelUnits) -> crate::Mesh {
    if matches!(kind, crate::Kind::Stl | crate::Kind::Obj | crate::Kind::Ply) {
        let factor = units.scale();
        if factor != 1.0 {
            mesh.scale(factor);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;
    use crate::config::ModelUnits;

    #[test]
    fn stl_obj_and_ply_follow_the_unit_of_the_files_others_do_not() {
        let cube = crate::Mesh::cube(10.0);
        let plain = in_units(Kind::Stl, cube.clone(), ModelUnits::Millimetres);
        assert_eq!(plain, cube, "millimetres: unchanged");
        let inch = in_units(Kind::Stl, cube.clone(), ModelUnits::Inches);
        let obj = in_units(Kind::Obj, cube.clone(), ModelUnits::Inches);
        let threemf = in_units(Kind::ThreeMf, cube.clone(), ModelUnits::Inches);
        assert!((inch.bounds().unwrap().size().x - 254.0).abs() < 1e-3);
        assert_eq!(obj, inch);
        assert_eq!(threemf, cube, "3MF declares its own unit");
    }
}
