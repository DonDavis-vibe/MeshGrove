//! User settings in `~/.config/stl-browser/config.toml`.

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
