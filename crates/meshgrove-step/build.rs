//! Compiles the OpenCascade glue. Without OCCT headers (OCCT_INCLUDE, default
//! /usr/include/opencascade) the mesher is built as a stub that reports missing STEP support,
//! so the workspace still builds everywhere. OCCT_LIB adds a library folder (e.g. Homebrew's).
//! OCCT_STATIC=1 links a static OpenCascade build (the release packages use vcpkg's): every
//! toolkit in OCCT_LIB goes to the linker, which keeps only what the mesher needs.

use std::path::Path;

/// The toolkits the glue calls into (STEP and STL exchange, meshing, a box primitive) and what
/// those need. OpenCascade 7.8 merged the data-exchange libraries into TKDE*.
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

/// The same for OpenCascade 7.5 to 7.7 (Ubuntu 22.04 and 24.04, Debian 12).
const OCCT_LIBS_BEFORE_7_8: &[&str] = &[
    "TKSTEP",
    "TKSTEPAttr",
    "TKSTEP209",
    "TKSTEPBase",
    "TKSTL",
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

const MIN_VERSION: (u32, u32) = (7, 5);

fn main() {
    println!("cargo:rustc-check-cfg=cfg(no_occt)");
    println!("cargo:rerun-if-changed=cpp/step_mesher.cpp");
    println!("cargo:rerun-if-env-changed=OCCT_INCLUDE");
    println!("cargo:rerun-if-env-changed=OCCT_LIB");
    println!("cargo:rerun-if-env-changed=OCCT_STATIC");
    let include = std::env::var("OCCT_INCLUDE").unwrap_or_else(|_| "/usr/include/opencascade".into());
    if !Path::new(&include).join("STEPControl_Reader.hxx").exists() {
        println!(
            "cargo:warning=OpenCascade headers not found in {include}: building meshgrove-step-mesher without STEP support"
        );
        println!("cargo:rustc-cfg=no_occt");
        return;
    }
    let version = occt_version(Path::new(&include));
    if version < MIN_VERSION {
        println!(
            "cargo:warning=OpenCascade {}.{} found, {}.{} or newer is needed: building meshgrove-step-mesher without STEP support",
            version.0, version.1, MIN_VERSION.0, MIN_VERSION.1
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
    let lib_dir = std::env::var("OCCT_LIB").ok();
    if let Some(dir) = &lib_dir {
        println!("cargo:rustc-link-search=native={dir}");
    }
    if std::env::var("OCCT_STATIC").is_ok_and(|v| v == "1") {
        let dir = lib_dir.expect("OCCT_STATIC=1 needs OCCT_LIB, the folder with the static OpenCascade libraries");
        link_static(Path::new(&dir));
        return;
    }
    let libs = if version < (7, 8) { OCCT_LIBS_BEFORE_7_8 } else { OCCT_LIBS };
    for lib in libs {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
}

/// Links all static OpenCascade toolkits in `dir`. The GNU linker resolves static libraries in
/// command-line order only, so on Linux they go into one group instead of a dependency order.
fn link_static(dir: &Path) {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let (prefix, suffix) = if target_os == "windows" { ("", ".lib") } else { ("lib", ".a") };
    let mut libs: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read OCCT_LIB {}: {e}", dir.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let lib = name.strip_prefix(prefix)?.strip_suffix(suffix)?;
            lib.starts_with("TK").then(|| lib.to_owned())
        })
        .collect();
    libs.sort();
    assert!(libs.iter().any(|l| l == "TKernel"), "no static OpenCascade libraries in {}", dir.display());
    if target_os == "linux" {
        println!("cargo:rustc-link-arg=-Wl,--start-group");
        for lib in &libs {
            println!("cargo:rustc-link-arg={}", dir.join(format!("lib{lib}.a")).display());
        }
        println!("cargo:rustc-link-arg=-Wl,--end-group");
        // After the group, so that the linker still sees what the toolkits need.
        for lib in ["stdc++", "pthread", "dl", "m"] {
            println!("cargo:rustc-link-arg=-l{lib}");
        }
    } else {
        for lib in &libs {
            println!("cargo:rustc-link-lib=static={lib}");
        }
    }
    if target_os == "windows" {
        // System libraries that OpenCascade's core uses.
        for lib in ["advapi32", "user32", "gdi32", "ws2_32", "psapi"] {
            println!("cargo:rustc-link-lib=dylib={lib}");
        }
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
