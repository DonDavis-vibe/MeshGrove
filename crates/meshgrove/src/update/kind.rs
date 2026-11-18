//! How this copy of Meshgrove was installed: the package the release workflow built it for
//! (`MESHGROVE_PACKAGE` at build time) and where it runs from. It decides which file of a release
//! fits, and whether Meshgrove can put the new version in place itself.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use semver::Version;

/// The package this build was made for: set by the release workflow per job, and to "source" by
/// `packaging/install.sh`. Unset (a plain `cargo build`), Meshgrove never looks for updates.
pub const PACKAGE: Option<&str> = option_env!("MESHGROVE_PACKAGE");

#[derive(Debug, Clone, PartialEq)]
pub enum InstallKind {
    /// Built without a package marker: never checks.
    Dev,
    /// The AppImage, at this path (`$APPIMAGE`).
    AppImage(PathBuf),
    /// The AppImage's program run unpacked (no `$APPIMAGE`): only the release page helps.
    Unpacked,
    /// A `.deb` built for this distribution ("ubuntu24.04", "debian13" …).
    Deb(String),
    /// An `.rpm` with this distribution tag ("fc44").
    Rpm(String),
    /// Installed with the Windows installer into `dir`; `all_users` under Program Files.
    WindowsSetup { dir: PathBuf, all_users: bool },
    /// The Windows ZIP, unpacked into this folder.
    WindowsPortable(PathBuf),
    /// The macOS app bundle at this path.
    MacApp(PathBuf),
    /// The macOS program outside an app bundle.
    MacOther,
    /// Built from the source with `packaging/install.sh`.
    Source,
}

/// What [`detect`] looks at, given from outside so tests can say what it finds.
pub struct Probe<'a> {
    pub env: &'a dyn Fn(&str) -> Option<OsString>,
    pub exists: &'a dyn Fn(&Path) -> bool,
    /// This program's path.
    pub exe: Option<PathBuf>,
}

impl Probe<'_> {
    /// This process: its environment, its files, its program.
    pub fn with_process<R>(f: impl FnOnce(&Probe) -> R) -> R {
        let env = |name: &str| std::env::var_os(name);
        let exists = |path: &Path| path.exists();
        let exe = std::env::current_exe().ok().map(|e| e.canonicalize().unwrap_or(e));
        f(&Probe { env: &env, exists: &exists, exe })
    }
}

pub fn detect(package: Option<&str>, probe: &Probe) -> InstallKind {
    let Some(package) = package.map(str::trim).filter(|p| !p.is_empty()) else { return InstallKind::Dev };
    if let Some(distro) = package.strip_prefix("deb-") {
        return InstallKind::Deb(distro.to_owned());
    }
    if let Some(dist) = package.strip_prefix("rpm-") {
        return InstallKind::Rpm(dist.to_owned());
    }
    match package {
        "source" => InstallKind::Source,
        "appimage" => match (probe.env)("APPIMAGE").filter(|p| !p.is_empty()) {
            Some(path) => InstallKind::AppImage(PathBuf::from(path)),
            None => InstallKind::Unpacked,
        },
        "windows" => {
            let Some(dir) = probe.exe.as_deref().and_then(Path::parent) else { return InstallKind::Dev };
            if (probe.exists)(&dir.join("unins000.exe")) {
                let all_users = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
                    .iter()
                    .filter_map(|v| (probe.env)(v))
                    .any(|root| starts_with_ignoring_case(dir, Path::new(&root)));
                InstallKind::WindowsSetup { dir: dir.to_owned(), all_users }
            } else {
                InstallKind::WindowsPortable(dir.to_owned())
            }
        }
        "macos" => probe
            .exe
            .as_deref()
            .and_then(|exe| exe.ancestors().find(|a| a.extension().is_some_and(|e| e == "app")))
            .map(|bundle| InstallKind::MacApp(bundle.to_owned()))
            .unwrap_or(InstallKind::MacOther),
        _ => InstallKind::Dev,
    }
}

/// Windows paths: `C:\Program Files\…` under `c:\program files`.
fn starts_with_ignoring_case(path: &Path, root: &Path) -> bool {
    let (path, root) = (path.to_string_lossy().to_lowercase(), root.to_string_lossy().to_lowercase());
    let root = root.trim_end_matches(['\\', '/']);
    !root.is_empty() && path.strip_prefix(root).is_some_and(|rest| rest.is_empty() || rest.starts_with(['\\', '/']))
}

impl InstallKind {
    /// Whether this copy looks for updates at all.
    pub fn checks(&self) -> bool {
        !matches!(self, InstallKind::Dev)
    }

    /// The release file that fits this installation, by the names the release workflow gives.
    pub fn asset_name(&self, v: &Version) -> Option<String> {
        Some(match self {
            InstallKind::AppImage(_) => format!("Meshgrove-{v}-x86_64.AppImage"),
            InstallKind::WindowsSetup { .. } => format!("Meshgrove-{v}-windows-x64-setup.exe"),
            InstallKind::WindowsPortable(_) => format!("Meshgrove-{v}-windows-x64-portable.zip"),
            InstallKind::MacApp(_) | InstallKind::MacOther => format!("Meshgrove-{v}-macos-universal.dmg"),
            InstallKind::Deb(distro) => format!("meshgrove_{v}-1_{distro}_amd64.deb"),
            InstallKind::Rpm(dist) => format!("meshgrove-{v}-1.{dist}.x86_64.rpm"),
            InstallKind::Dev | InstallKind::Unpacked | InstallKind::Source => return None,
        })
    }

    /// Meshgrove can put the new version in place itself (or start what does it).
    pub fn one_click(&self) -> bool {
        matches!(
            self,
            InstallKind::AppImage(_)
                | InstallKind::WindowsSetup { .. }
                | InstallKind::WindowsPortable(_)
                | InstallKind::MacApp(_)
        )
    }

    /// The command that updates this installation, for the user to copy: the package manager's,
    /// or the source's.
    pub fn command(&self, v: &Version) -> Option<String> {
        let file = self.asset_name(v);
        match self {
            InstallKind::Deb(_) => file.map(|f| format!("sudo apt install ./{f}")),
            InstallKind::Rpm(_) => file.map(|f| format!("sudo dnf install ./{f}")),
            InstallKind::Source => Some("git pull && packaging/install.sh".to_owned()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn kind(package: Option<&str>, env: &[(&str, &str)], files: &[&str], exe: &str) -> InstallKind {
        let env: HashMap<String, OsString> = env.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        let lookup = |name: &str| env.get(name).cloned();
        let exists = |p: &Path| files.iter().any(|f| f == p);
        detect(package, &Probe { env: &lookup, exists: &exists, exe: Some(PathBuf::from(exe)) })
    }

    #[test]
    fn a_plain_build_never_checks() {
        assert_eq!(kind(None, &[("APPIMAGE", "/a.AppImage")], &[], "/x/meshgrove"), InstallKind::Dev);
        assert_eq!(kind(Some(" "), &[], &[], "/x/meshgrove"), InstallKind::Dev);
        assert_eq!(kind(Some("flatpak"), &[], &[], "/x/meshgrove"), InstallKind::Dev);
        assert!(!InstallKind::Dev.checks());
    }

    #[test]
    fn tells_the_linux_packages_apart() {
        let app = "/home/u/Apps/Meshgrove-0.12.0-x86_64.AppImage";
        assert_eq!(
            kind(Some("appimage"), &[("APPIMAGE", app)], &[], "/tmp/.mount/usr/bin/meshgrove"),
            InstallKind::AppImage(app.into())
        );
        assert_eq!(kind(Some("appimage"), &[], &[], "/tmp/x/usr/bin/meshgrove"), InstallKind::Unpacked);
        assert_eq!(
            kind(Some("deb-ubuntu24.04"), &[], &[], "/usr/bin/meshgrove"),
            InstallKind::Deb("ubuntu24.04".into())
        );
        assert_eq!(kind(Some("rpm-fc44"), &[], &[], "/usr/bin/meshgrove"), InstallKind::Rpm("fc44".into()));
        assert_eq!(kind(Some("source"), &[], &[], "/home/u/.local/bin/meshgrove"), InstallKind::Source);
    }

    #[test]
    fn tells_the_windows_installer_from_the_zip() {
        let env = [("ProgramFiles", r"C:\Program Files"), ("ProgramFiles(x86)", r"C:\Program Files (x86)")];
        let per_user = r"C:\Users\u\AppData\Local\Programs\Meshgrove";
        let setup = |dir: &str| {
            kind(Some("windows"), &env, &[&format!(r"{dir}\unins000.exe")], &format!(r"{dir}\meshgrove.exe"))
        };
        // (Backslashes only separate on Windows; the parent of the exe is all that counts.)
        if cfg!(windows) {
            assert_eq!(setup(per_user), InstallKind::WindowsSetup { dir: per_user.into(), all_users: false });
            let all = r"c:\program files\Meshgrove";
            assert_eq!(setup(all), InstallKind::WindowsSetup { dir: all.into(), all_users: true });
            let zip = r"D:\Tools\Meshgrove";
            assert_eq!(
                kind(Some("windows"), &env, &[], &format!(r"{zip}\meshgrove.exe")),
                InstallKind::WindowsPortable(zip.into())
            );
        }
        assert!(starts_with_ignoring_case(Path::new(r"C:\PROGRAM FILES\Meshgrove"), Path::new(r"c:\program files\")));
        assert!(!starts_with_ignoring_case(Path::new(r"C:\Program Files (x86)\M"), Path::new(r"C:\Program Files")));
        let unix = kind(Some("windows"), &[], &["/opt/m/unins000.exe"], "/opt/m/meshgrove.exe");
        assert_eq!(unix, InstallKind::WindowsSetup { dir: "/opt/m".into(), all_users: false });
    }

    #[test]
    fn finds_the_macos_bundle() {
        let exe = "/Applications/Meshgrove.app/Contents/MacOS/meshgrove";
        assert_eq!(kind(Some("macos"), &[], &[], exe), InstallKind::MacApp("/Applications/Meshgrove.app".into()));
        assert_eq!(kind(Some("macos"), &[], &[], "/usr/local/bin/meshgrove"), InstallKind::MacOther);
    }

    #[test]
    fn names_the_file_of_each_package() {
        let v = Version::new(0, 13, 0);
        let names = [
            (InstallKind::AppImage("/a".into()), Some("Meshgrove-0.13.0-x86_64.AppImage")),
            (
                InstallKind::WindowsSetup { dir: "/d".into(), all_users: false },
                Some("Meshgrove-0.13.0-windows-x64-setup.exe"),
            ),
            (InstallKind::WindowsPortable("/d".into()), Some("Meshgrove-0.13.0-windows-x64-portable.zip")),
            (InstallKind::MacApp("/A.app".into()), Some("Meshgrove-0.13.0-macos-universal.dmg")),
            (InstallKind::Deb("debian13".into()), Some("meshgrove_0.13.0-1_debian13_amd64.deb")),
            (InstallKind::Rpm("fc44".into()), Some("meshgrove-0.13.0-1.fc44.x86_64.rpm")),
            (InstallKind::Source, None),
            (InstallKind::Unpacked, None),
        ];
        for (kind, name) in names {
            assert_eq!(kind.asset_name(&v).as_deref(), name, "{kind:?}");
        }
        assert_eq!(
            InstallKind::Rpm("fc44".into()).command(&v).unwrap(),
            "sudo dnf install ./meshgrove-0.13.0-1.fc44.x86_64.rpm"
        );
        assert_eq!(InstallKind::Source.command(&v).unwrap(), "git pull && packaging/install.sh");
        assert!(InstallKind::AppImage("/a".into()).one_click() && !InstallKind::Deb("x".into()).one_click());
    }
}
