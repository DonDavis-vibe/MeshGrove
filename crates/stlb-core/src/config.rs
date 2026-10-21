//! User settings in `~/.config/stl-browser/config.toml`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How to start the slicer with model files: `program [args…] files…`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slicer {
    pub name: String,
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment; `LD_LIBRARY_PATH` is prepended to the inherited value.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub workdir: Option<PathBuf>,
}

impl Slicer {
    /// Finds an installed slicer: the user's Anycubic Slicer Next build first, then common
    /// Orca/Prusa/Bambu installs (distribution packages and Flatpaks).
    pub fn detect() -> Option<Slicer> {
        let home = dirs::home_dir()?;
        let anycubic = home.join("opt/AnycubicSlicer");
        let anycubic_bin = anycubic.join("app/bin/AnycubicSlicerNext");
        if anycubic_bin.is_file() {
            return Some(Slicer {
                name: "Anycubic Slicer Next".into(),
                program: anycubic_bin,
                args: Vec::new(),
                env: vec![
                    ("LD_LIBRARY_PATH".into(), anycubic.join("app/lib").to_string_lossy().into_owned()),
                    // WebKitGTK renders a blank window on NVIDIA + Wayland without this.
                    ("WEBKIT_DISABLE_DMABUF_RENDERER".into(), "1".into()),
                ],
                workdir: Some(anycubic),
            });
        }
        let flatpak = |id: &str| {
            ["/var/lib/flatpak/exports/bin", &format!("{}/.local/share/flatpak/exports/bin", home.display())]
                .iter()
                .map(|dir| Path::new(dir).join(id))
                .find(|p| p.exists())
        };
        let candidates = [
            ("OrcaSlicer", Some(PathBuf::from("/usr/bin/orca-slicer"))),
            ("OrcaSlicer", flatpak("io.github.softfever.OrcaSlicer")),
            ("PrusaSlicer", Some(PathBuf::from("/usr/bin/prusa-slicer"))),
            ("PrusaSlicer", flatpak("com.prusa3d.PrusaSlicer")),
            ("Bambu Studio", flatpak("com.bambulab.BambuStudio")),
        ];
        candidates.into_iter().find_map(|(name, program)| {
            let program = program.filter(|p| p.exists())?;
            Some(Slicer { name: name.into(), program, args: Vec::new(), env: Vec::new(), workdir: None })
        })
    }
}

/// Gallery sort order (folders always come first).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Size,
    Type,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Folders indexed as the library and shown under "Bibliothek" in the sidebar.
    pub library_roots: Vec<PathBuf>,
    pub last_folder: Option<PathBuf>,
    /// Tile edge length in logical pixels.
    pub tile_size: f32,
    pub only_models: bool,
    pub sort_by: SortKey,
    pub sort_descending: bool,
    /// "de" / "en"; `None` follows the system language.
    pub language: Option<String>,
    /// `None`: detect an installed slicer at startup.
    pub slicer: Option<Slicer>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            library_roots: Vec::new(),
            last_folder: None,
            tile_size: 160.0,
            only_models: false,
            sort_by: SortKey::Name,
            sort_descending: false,
            language: None,
            slicer: None,
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("stl-browser").join("config.toml"))
    }

    /// Missing file → defaults. A broken file is reported so the caller can warn instead of overwriting silently.
    pub fn load() -> Result<Config, String> {
        let Some(path) = Self::path() else {
            return Ok(Config::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let path = Self::path().ok_or_else(|| io::Error::other("no config directory"))?;
        std::fs::create_dir_all(path.parent().expect("config file has a parent"))?;
        std::fs::write(path, toml::to_string_pretty(self).map_err(io::Error::other)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_fill_in_defaults() {
        let cfg: Config = toml::from_str("library_roots = [\"/data/3D_druck\"]\n").unwrap();
        assert_eq!(cfg.library_roots, vec![PathBuf::from("/data/3D_druck")]);
        assert_eq!(cfg.tile_size, 160.0);
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), cfg);
    }
}
