//! Compiles the OpenCascade glue. Without OCCT headers (OCCT_INCLUDE, default
//! /usr/include/opencascade) the mesher is built as a stub that reports missing STEP support,
//! so the workspace still builds everywhere. OCCT_LIB adds a library folder (e.g. Homebrew's).

use std::path::Path;

const OCCT_LIBS: &[&str] = &[
    "TKDESTEP",
    "TKDESTL",
    "TKDE",
    "TKXSBase",
    "TKMesh",
    "TKPrim",
    "TKTopAlgo",
    "TKGeomAlgo",
    "TKGeomBase",
    "TKBRep",
    "TKG3d",
    "TKG2d",
    "TKMath",
    "TKernel",
];

fn main() {
    println!("cargo:rustc-check-cfg=cfg(no_occt)");
    println!("cargo:rerun-if-changed=cpp/step_mesher.cpp");
    println!("cargo:rerun-if-env-changed=OCCT_INCLUDE");
    println!("cargo:rerun-if-env-changed=OCCT_LIB");
    let include = std::env::var("OCCT_INCLUDE").unwrap_or_else(|_| "/usr/include/opencascade".into());
    if !Path::new(&include).join("STEPControl_Reader.hxx").exists() {
        println!(
            "cargo:warning=OpenCascade headers not found in {include}: building meshgrove-step-mesher without STEP support"
        );
        println!("cargo:rustc-cfg=no_occt");
        return;
    }
    // The data-exchange libraries were split and renamed (TKDESTEP, …) in 7.8.
    let version = occt_version(Path::new(&include));
    if version < (7, 8) {
        println!(
            "cargo:warning=OpenCascade {}.{} found, 7.8 or newer is needed: building meshgrove-step-mesher without STEP support",
            version.0, version.1
        );
        println!("cargo:rustc-cfg=no_occt");
        return;
    }
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("cpp/step_mesher.cpp")
        .include(&include)
        .warnings(false)
        .compile("meshgrove_step_glue");
    if let Ok(dir) = std::env::var("OCCT_LIB") {
        println!("cargo:rustc-link-search=native={dir}");
    }
    for lib in OCCT_LIBS {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
}

/// (major, minor) from Standard_Version.hxx; (0, 0) if it cannot be read.
fn occt_version(include: &Path) -> (u32, u32) {
    let header = std::fs::read_to_string(include.join("Standard_Version.hxx")).unwrap_or_default();
    let define = |name: &str| {
        header.lines().find_map(|line| line.strip_prefix("#define ")?.strip_prefix(name)?.trim().parse().ok())
    };
    (define("OCC_VERSION_MAJOR").unwrap_or(0), define("OCC_VERSION_MINOR").unwrap_or(0))
}
