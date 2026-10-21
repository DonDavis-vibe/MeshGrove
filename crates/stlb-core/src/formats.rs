use std::path::Path;

/// What the browser knows how to show for a directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Folder,
    Stl,
    ThreeMf,
    Step,
    Archive,
    Image,
    Pdf,
    Other,
}

impl Kind {
    pub fn from_path(path: &Path) -> Kind {
        if path.is_dir() {
            return Kind::Folder;
        }
        Self::from_file_name(path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
    }

    /// Classifies by extension only (no filesystem access), so it also works for archive entries.
    pub fn from_file_name(name: &str) -> Kind {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".tar.gz") || lower.ends_with(".tar.xz") || lower.ends_with(".tar.bz2") {
            return Kind::Archive;
        }
        let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        match ext {
            "stl" => Kind::Stl,
            "3mf" => Kind::ThreeMf,
            "step" | "stp" => Kind::Step,
            "zip" | "7z" | "rar" | "tar" | "tgz" => Kind::Archive,
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" => Kind::Image,
            "pdf" => Kind::Pdf,
            _ => Kind::Other,
        }
    }

    /// Files that become a 3D mesh (rendered thumbnail, viewer, hover turntable, duplicate geometry).
    pub fn is_model(self) -> bool {
        matches!(self, Kind::Stl | Kind::ThreeMf | Kind::Step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension_case_insensitively() {
        assert_eq!(Kind::from_file_name("Dragon.STL"), Kind::Stl);
        assert_eq!(Kind::from_file_name("plate.3mf"), Kind::ThreeMf);
        assert_eq!(Kind::from_file_name("bracket.stp"), Kind::Step);
        assert_eq!(Kind::from_file_name("pack.tar.gz"), Kind::Archive);
        assert_eq!(Kind::from_file_name("Archiv.zip"), Kind::Archive);
        assert_eq!(Kind::from_file_name("IMG_20260704.jpg"), Kind::Image);
        assert_eq!(Kind::from_file_name("Regelwerk.pdf"), Kind::Pdf);
        assert_eq!(Kind::from_file_name("notes"), Kind::Other);
        assert!(Kind::Step.is_model() && !Kind::Archive.is_model());
    }
}
