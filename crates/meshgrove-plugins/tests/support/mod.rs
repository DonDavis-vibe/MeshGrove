//! What the tests of the plugin host share.
#![allow(dead_code)]

use std::io::Write;

use meshgrove_plugin_api::MeshData;

/// A box as 3MF `<object>`: `extra` goes into its tag, `triangle` into the tag of every triangle.
fn object(id: u32, extra: &str, size: [f32; 3], triangle: &str) -> String {
    let mesh = MeshData::cuboid(size);
    let vertices: String = mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .map(|[x, y, z]| format!("<vertex x=\"{x}\" y=\"{y}\" z=\"{z}\"/>"))
        .collect();
    let triangles: String = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|[a, b, c]| format!("<triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"{triangle}/>"))
        .collect();
    format!(
        "<object id=\"{id}\" type=\"model\" {extra}><mesh><vertices>{vertices}</vertices>\
         <triangles>{triangles}</triangles></mesh></object>"
    )
}

fn model(resources: &str, build: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <model unit=\"millimeter\" xml:lang=\"en-US\" \
         xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\" \
         xmlns:p=\"http://schemas.microsoft.com/3dmanufacturing/production/2015/06\">\n\
         <metadata name=\"Application\">Meshgrove tests</metadata>\n\
         <resources>{resources}</resources>\n<build>{build}</build>\n</model>\n"
    )
    .into_bytes()
}

/// A 3MF package of these files.
pub fn package(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in files {
        zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// A plain 3MF of two boxes: a "Dose" of 40 × 30 × 20 mm at the origin and a "Deckel" of
/// 40 × 30 × 2 mm beside it (50 mm along x), which is placed twice.
pub fn dose_3mf() -> Vec<u8> {
    let resources = format!(
        "{}{}",
        object(1, "name=\"Dose\"", [40.0, 30.0, 20.0], ""),
        object(2, "name=\"Deckel\"", [40.0, 30.0, 2.0], "")
    );
    let build = "<item objectid=\"1\"/><item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 50 0 0\"/>\
                 <item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 100 0 0\"/>";
    package(&[("3D/3dmodel.model", model(&resources, build))])
}

/// A box as binary STL.
pub fn stl(size: [f32; 3]) -> Vec<u8> {
    MeshData::cuboid(size).to_stl("box")
}
