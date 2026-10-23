//! Finding the slicers installed on this computer: the ten most common FDM slicers and ten
//! resin (SLA/MSLA) slicers, in every way they get installed (a distribution package, a Flatpak,
//! a Snap, an AppImage, a vendor's folder in /opt on Linux; the usual program folders on
//! Windows; application bundles on macOS). The settings list what was found and the user adds
//! the ones they want.

use std::path::{Path, PathBuf};

use crate::config::{Program, Slicer, plain, subdirs};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlicerKind {
    /// Filament printers.
    Fdm,
    /// Resin printers.
    Sla,
}

/// How a slicer was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// A distribution package, a vendor's folder or the program folders of Windows and macOS.
    Native,
    Flatpak,
    Snap,
    AppImage,
}

impl Variant {
    /// Added to the slicer's name when it is not the native install: "OrcaSlicer (Flatpak)".
    fn suffix(self) -> &'static str {
        match self {
            Variant::Native => "",
            Variant::Flatpak => " (Flatpak)",
            Variant::Snap => " (Snap)",
            Variant::AppImage => " (AppImage)",
        }
    }
}

/// A slicer found on this computer, ready to be added to the settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Detected {
    /// Slicer name and how it is installed: "OrcaSlicer (Flatpak)".
    pub name: String,
    pub kind: SlicerKind,
    pub variant: Variant,
    pub program: Slicer,
}

/// What is known about one slicer. Names are matched case-insensitively where they are
/// "keys" (file names of AppImages and bundles with spaces, dashes and underscores removed).
#[allow(dead_code)] // each platform reads only its own fields
struct Def {
    name: &'static str,
    kind: SlicerKind,
    /// Linux: program names in the usual bin folders, `$PATH` and the vendors' folders in /opt.
    bins: &'static [&'static str],
    /// Linux: Flatpak application ids.
    flatpak: &'static [&'static str],
    /// Linux: Snap names.
    snap: &'static [&'static str],
    /// Linux: keys of AppImage file names.
    appimage: &'static [&'static str],
    /// Windows: lower-case file names of the program.
    win: &'static [&'static str],
    /// macOS: keys of application bundle names.
    mac: &'static [&'static str],
}

const fn def(
    name: &'static str,
    kind: SlicerKind,
    bins: &'static [&'static str],
    flatpak: &'static [&'static str],
    appimage: &'static [&'static str],
    win: &'static [&'static str],
    mac: &'static [&'static str],
) -> Def {
    Def { name, kind, bins, flatpak, snap: &[], appimage, win, mac }
}

use SlicerKind::{Fdm, Sla};

/// The catalogue, the first entries being the ones a new user most likely wants as default.
const CATALOGUE: &[Def] = &[
    def(
        "Anycubic Slicer Next",
        Fdm,
        &["AnycubicSlicerNext"],
        &[],
        &["anycubicslicer"],
        &["anycubicslicernext.exe"],
        &["anycubicslicer"],
    ),
    def(
        "OrcaSlicer",
        Fdm,
        &["orca-slicer", "OrcaSlicer"],
        &["io.github.softfever.OrcaSlicer"],
        &["orcaslicer"],
        &["orca-slicer.exe"],
        &["orcaslicer"],
    ),
    def(
        "PrusaSlicer",
        Fdm,
        &["prusa-slicer", "PrusaSlicer"],
        &["com.prusa3d.PrusaSlicer"],
        &["prusaslicer"],
        &["prusa-slicer.exe"],
        &["prusaslicer"],
    ),
    def(
        "Bambu Studio",
        Fdm,
        &["bambu-studio", "BambuStudio"],
        &["com.bambulab.BambuStudio"],
        &["bambustudio"],
        &["bambu-studio.exe"],
        &["bambustudio"],
    ),
    def(
        "UltiMaker Cura",
        Fdm,
        &["cura", "UltiMaker-Cura"],
        &["com.ultimaker.cura"],
        &["ultimakercura", "cura"],
        &["ultimaker-cura.exe", "cura.exe"],
        &["ultimakercura"],
    ),
    def(
        "Creality Print",
        Fdm,
        &["creality-print", "CrealityPrint"],
        &[],
        &["crealityprint"],
        &["crealityprint.exe"],
        &["crealityprint"],
    ),
    def(
        "SuperSlicer",
        Fdm,
        &["superslicer", "SuperSlicer"],
        &["net.supermerill.SuperSlicer"],
        &["superslicer"],
        &["superslicer.exe"],
        &["superslicer"],
    ),
    def("Simplify3D", Fdm, &["Simplify3D"], &[], &[], &["simplify3d.exe"], &["simplify3d"]),
    def("IdeaMaker", Fdm, &["ideamaker"], &[], &["ideamaker"], &["ideamaker.exe"], &["ideamaker"]),
    def(
        "ElegooSlicer",
        Fdm,
        &["elegoo-slicer", "ElegooSlicer"],
        &[],
        &["elegooslicer"],
        &["elegooslicer.exe"],
        &["elegooslicer"],
    ),
    def(
        "Lychee Slicer",
        Sla,
        &["lychee-slicer", "LycheeSlicer"],
        &[],
        &["lycheeslicer"],
        &["lychee slicer.exe", "lycheeslicer.exe"],
        &["lycheeslicer"],
    ),
    // Listed before ChiTuBox: its bundle name contains the other's.
    def(
        "ChiTuBox Pro",
        Sla,
        &["chitubox-pro", "CHITUBOXPro"],
        &[],
        &["chituboxpro"],
        &["chitubox pro.exe", "chituboxpro.exe"],
        &["chituboxpro"],
    ),
    def("ChiTuBox", Sla, &["chitubox", "CHITUBOX"], &[], &["chitubox"], &["chitubox.exe"], &["chitubox"]),
    def(
        "Photon Workshop",
        Sla,
        &["PhotonWorkshop", "photon-workshop"],
        &[],
        &["photonworkshop"],
        &["photonworkshop.exe"],
        &["photonworkshop"],
    ),
    def(
        "Voxeldance Tango",
        Sla,
        &["VoxelDance", "voxeldance"],
        &[],
        &[],
        &["voxeldance tango.exe", "voxeldancetango.exe"],
        &["voxeldance"],
    ),
    def("UVtools", Sla, &["uvtools", "UVtools"], &[], &["uvtools"], &["uvtools.exe"], &["uvtools"]),
    def("PreForm", Sla, &["PreForm"], &[], &[], &["preform.exe"], &["preform"]),
    def("Halot Box", Sla, &["HALOT-BOX", "halotbox"], &[], &[], &["halotbox.exe", "halot-box.exe"], &["halotbox"]),
    def("NovaMaker", Sla, &["NovaMaker"], &[], &[], &["novamaker.exe"], &["novamaker"]),
    def("Asiga Composer", Sla, &[], &[], &[], &["asigacomposer.exe"], &["asigacomposer"]),
];

/// Every catalogue slicer installed here, each in every way it is installed. The same program
/// is listed once.
pub fn detect_all() -> Vec<Detected> {
    let mut found = platform_detect();
    let mut seen = Vec::new();
    found.retain(|d: &Detected| {
        let fresh = !seen.contains(&d.program.program);
        seen.push(d.program.program.clone());
        fresh
    });
    found
}

fn detected(def: &Def, variant: Variant, program: Program) -> Detected {
    let name = format!("{}{}", def.name, variant.suffix());
    Detected { program: Program { name: name.clone(), ..program }, name, kind: def.kind, variant }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// Where slicers are looked for; a struct so tests can point it at a temporary home.
    pub struct Places {
        pub home: PathBuf,
        pub bin_dirs: Vec<PathBuf>,
        /// Folders whose subfolders hold vendor installs (`/opt/PreForm/PreForm`).
        pub opt_roots: Vec<PathBuf>,
        pub flatpak_dirs: Vec<PathBuf>,
        pub snap_dir: PathBuf,
        pub appimage_dirs: Vec<PathBuf>,
    }

    impl Places {
        pub fn real() -> Option<Places> {
            let home = dirs::home_dir()?;
            let mut bin_dirs: Vec<PathBuf> =
                std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
            for dir in ["/usr/bin", "/usr/local/bin"]
                .map(PathBuf::from)
                .into_iter()
                .chain([home.join(".local/bin"), home.join("bin")])
            {
                if !bin_dirs.contains(&dir) {
                    bin_dirs.push(dir);
                }
            }
            Some(Places {
                opt_roots: vec![PathBuf::from("/opt"), home.join("opt")],
                flatpak_dirs: vec![
                    PathBuf::from("/var/lib/flatpak/exports/bin"),
                    home.join(".local/share/flatpak/exports/bin"),
                ],
                snap_dir: PathBuf::from("/snap/bin"),
                appimage_dirs: [
                    "Applications",
                    ".local/bin",
                    "Downloads",
                    "opt",
                    "AppImages",
                    "Desktop",
                    "bin",
                    "Apps",
                ]
                .iter()
                .map(|d| home.join(d))
                .collect(),
                bin_dirs,
                home,
            })
        }
    }

    fn executable(path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }

    /// A program by name in the bin folders, or directly in (or in the `bin` of) a vendor's
    /// folder below /opt.
    fn native(places: &Places, bins: &[&str]) -> Option<PathBuf> {
        for bin in bins {
            if let Some(p) = places.bin_dirs.iter().map(|d| d.join(bin)).find(|p| executable(p)) {
                return Some(p);
            }
            for root in &places.opt_roots {
                let mut dirs = subdirs(root);
                dirs.sort();
                for dir in dirs {
                    if let Some(p) = [dir.join(bin), dir.join("bin").join(bin)].into_iter().find(|p| executable(p)) {
                        return Some(p);
                    }
                }
            }
        }
        None
    }

    /// The newest (last by name) AppImage whose name contains one of the keys.
    fn appimage(places: &Places, keys: &[&str]) -> Option<PathBuf> {
        let mut best: Option<PathBuf> = None;
        for dir in &places.appimage_dirs {
            let mut files: Vec<PathBuf> =
                std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
            files.sort();
            for file in files.into_iter().rev() {
                let name = file.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                let key = name.replace([' ', '-', '_'], "");
                if name.ends_with(".appimage") && keys.iter().any(|k| key.contains(k)) && executable(&file) {
                    best = Some(file);
                    break;
                }
            }
            if best.is_some() {
                break;
            }
        }
        best
    }

    /// Anycubic Slicer Next unpacked into `~/opt/AnycubicSlicer`, which needs its own libraries
    /// and (WebKitGTK renders a blank window on NVIDIA + Wayland otherwise) a renderer switch.
    fn anycubic_unpacked(home: &Path) -> Option<Program> {
        let root = home.join("opt/AnycubicSlicer");
        let bin = root.join("app/bin/AnycubicSlicerNext");
        bin.is_file().then(|| Program {
            name: String::new(),
            program: bin,
            args: Vec::new(),
            env: vec![
                ("LD_LIBRARY_PATH".into(), root.join("app/lib").to_string_lossy().into_owned()),
                ("WEBKIT_DISABLE_DMABUF_RENDERER".into(), "1".into()),
            ],
            workdir: Some(root),
        })
    }

    pub fn detect(places: &Places) -> Vec<Detected> {
        let mut found = Vec::new();
        for def in CATALOGUE {
            if def.name == "Anycubic Slicer Next"
                && let Some(program) = anycubic_unpacked(&places.home)
            {
                found.push(detected(def, Variant::Native, program));
            }
            if let Some(path) = native(places, def.bins) {
                found.push(detected(def, Variant::Native, plain(def.name, path)));
            }
            for id in def.flatpak {
                if let Some(path) = places.flatpak_dirs.iter().map(|d| d.join(id)).find(|p| p.exists()) {
                    found.push(detected(def, Variant::Flatpak, plain(def.name, path)));
                }
            }
            for name in def.snap {
                let path = places.snap_dir.join(name);
                if path.exists() {
                    found.push(detected(def, Variant::Snap, plain(def.name, path)));
                }
            }
            if let Some(path) = appimage(places, def.appimage) {
                found.push(detected(def, Variant::AppImage, plain(def.name, path)));
            }
        }
        found
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn touch(path: &Path, executable: bool) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
            let mode = if executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        }

        fn places(root: &Path) -> Places {
            Places {
                home: root.join("home"),
                bin_dirs: vec![root.join("usr/bin")],
                opt_roots: vec![root.join("opt")],
                flatpak_dirs: vec![root.join("flatpak")],
                snap_dir: root.join("snap"),
                appimage_dirs: vec![root.join("home/Applications")],
            }
        }

        #[test]
        fn finds_every_way_a_slicer_is_installed() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            touch(&root.join("usr/bin/orca-slicer"), true);
            touch(&root.join("flatpak/io.github.softfever.OrcaSlicer"), true);
            touch(&root.join("home/Applications/OrcaSlicer_Linux_V2.3.0.AppImage"), true);
            touch(&root.join("home/Applications/OrcaSlicer_Linux_V2.2.0.AppImage"), true);
            touch(&root.join("opt/Simplify3D-5.1.2/Simplify3D"), true);
            touch(&root.join("opt/PreForm/bin/PreForm"), true);
            touch(&root.join("home/Applications/Lychee-Slicer.AppImage"), false); // not executable
            let found = detect(&places(root));
            let names: Vec<&str> = found.iter().map(|d| d.name.as_str()).collect();
            assert_eq!(names, ["OrcaSlicer", "OrcaSlicer (Flatpak)", "OrcaSlicer (AppImage)", "Simplify3D", "PreForm"]);
            let appimage = found.iter().find(|d| d.variant == Variant::AppImage).unwrap();
            assert!(appimage.program.program.ends_with("OrcaSlicer_Linux_V2.3.0.AppImage"), "the newest one");
            assert_eq!(found.iter().find(|d| d.name == "PreForm").unwrap().kind, SlicerKind::Sla);
            assert_eq!(found[0].program.name, "OrcaSlicer");
        }

        #[test]
        fn an_unpacked_anycubic_slicer_keeps_its_libraries() {
            let dir = tempfile::tempdir().unwrap();
            touch(&dir.path().join("home/opt/AnycubicSlicer/app/bin/AnycubicSlicerNext"), true);
            let found = detect(&places(dir.path()));
            let anycubic = found.iter().find(|d| d.name == "Anycubic Slicer Next").unwrap();
            assert!(anycubic.program.env.iter().any(|(k, _)| k == "LD_LIBRARY_PATH"));
            assert!(anycubic.program.workdir.is_some());
        }

        #[test]
        fn nothing_installed_finds_nothing() {
            let dir = tempfile::tempdir().unwrap();
            assert!(detect(&places(dir.path())).is_empty());
        }
    }
}

#[cfg(target_os = "linux")]
fn platform_detect() -> Vec<Detected> {
    linux::Places::real().map(|p| linux::detect(&p)).unwrap_or_default()
}

/// The first of the slicer's Windows file names among `files` (lower-case name, path).
#[cfg_attr(not(windows), allow(dead_code))]
fn pick_windows(def: &Def, files: &[(String, PathBuf)]) -> Option<Detected> {
    let (_, path) = def.win.iter().find_map(|exe| files.iter().find(|(name, _)| name == exe))?;
    Some(detected(def, Variant::Native, plain(def.name, path.clone())))
}

/// The slicer's macOS bundle among `bundles` (name without spaces, dashes and underscores in
/// lower case, path).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn pick_mac(def: &Def, bundles: &[(String, PathBuf)]) -> Option<Detected> {
    let (_, path) = def.mac.iter().find_map(|key| bundles.iter().find(|(stem, _)| stem.contains(key)))?;
    Some(detected(def, Variant::Native, plain(def.name, path.clone())))
}

#[cfg(windows)]
fn platform_detect() -> Vec<Detected> {
    // Installers pick different folders: look one and two levels below the program folders.
    let mut folders = Vec::new();
    for root in crate::config::program_folders() {
        for dir in subdirs(&root) {
            let below = subdirs(&dir);
            folders.push(dir);
            folders.extend(below);
        }
    }
    let files: Vec<(String, PathBuf)> = folders
        .iter()
        .flat_map(|f| std::fs::read_dir(f).into_iter().flatten().flatten())
        .map(|e| (e.file_name().to_string_lossy().to_lowercase(), e.path()))
        .collect();
    CATALOGUE.iter().filter_map(|def| pick_windows(def, &files)).collect()
}

#[cfg(target_os = "macos")]
fn platform_detect() -> Vec<Detected> {
    let bundles: Vec<(String, PathBuf)> = crate::config::app_folders()
        .iter()
        .flat_map(|f| std::fs::read_dir(f).into_iter().flatten().flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "app"))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().to_lowercase().replace([' ', '-', '_'], ""), p)))
        .collect();
    CATALOGUE.iter().filter_map(|def| pick_mac(def, &bundles)).collect()
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn platform_detect() -> Vec<Detected> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_name(name: &str) -> &'static Def {
        CATALOGUE.iter().find(|d| d.name == name).unwrap()
    }

    #[test]
    fn windows_programs_and_mac_bundles_are_matched_by_name() {
        let files = vec![
            ("orca-slicer.exe".to_owned(), PathBuf::from("C:/OrcaSlicer/orca-slicer.exe")),
            ("lychee slicer.exe".to_owned(), PathBuf::from("C:/Lychee Slicer/Lychee Slicer.exe")),
        ];
        assert_eq!(pick_windows(by_name("OrcaSlicer"), &files).unwrap().name, "OrcaSlicer");
        assert_eq!(pick_windows(by_name("Lychee Slicer"), &files).unwrap().kind, SlicerKind::Sla);
        assert!(pick_windows(by_name("PrusaSlicer"), &files).is_none());
        let bundles = vec![
            ("chituboxpro".to_owned(), PathBuf::from("/Applications/CHITUBOX Pro.app")),
            ("safari".to_owned(), PathBuf::from("/Applications/Safari.app")),
        ];
        assert_eq!(pick_mac(by_name("ChiTuBox Pro"), &bundles).unwrap().name, "ChiTuBox Pro");
        // The basic ChiTuBox matches the Pro bundle too; the list shows each program once.
        let mut all: Vec<Detected> = CATALOGUE.iter().filter_map(|d| pick_mac(d, &bundles)).collect();
        let mut seen = Vec::new();
        all.retain(|d| {
            let fresh = !seen.contains(&d.program.program);
            seen.push(d.program.program.clone());
            fresh
        });
        assert_eq!(all.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["ChiTuBox Pro"]);
    }

    #[test]
    fn the_catalogue_has_ten_of_each_and_unique_names() {
        assert_eq!(CATALOGUE.iter().filter(|d| d.kind == SlicerKind::Fdm).count(), 10);
        assert_eq!(CATALOGUE.iter().filter(|d| d.kind == SlicerKind::Sla).count(), 10);
        let mut names: Vec<&str> = CATALOGUE.iter().map(|d| d.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 20);
        for d in CATALOGUE {
            assert!(d.win.iter().all(|e| e.to_lowercase() == *e), "{}: Windows names are lower-case", d.name);
            assert!(d.appimage.iter().chain(d.mac).all(|k| !k.contains([' ', '-', '_']) && k.to_lowercase() == *k));
        }
    }
}
