//! Gives meshgrove.exe its icon in Explorer and the taskbar.

fn main() {
    // The translations are compiled in (rust-i18n): edit a locale file, get a new build.
    println!("cargo:rerun-if-changed=locales");
    println!("cargo:rerun-if-changed=../../packaging/icons/meshgrove.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../packaging/icons/meshgrove.ico");
        resource.set("ProductName", "Meshgrove");
        resource.set("FileDescription", "Meshgrove - gallery for 3D-print files");
        resource.set("CompanyName", "German Redneck Engineering");
        resource.set("LegalCopyright", "Davis, German Redneck Engineering - GPL-3.0-or-later");
        if let Err(e) = resource.compile() {
            println!("cargo:warning=no icon for meshgrove.exe: {e}");
        }
    }
}
