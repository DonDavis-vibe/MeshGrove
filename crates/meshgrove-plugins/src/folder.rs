//! A plain folder as a library ([`FolderLibrary`]): for `meshgrove-plugin-run --library` and
//! for tests, where there is no index and no settings of Meshgrove's. Ids are as in Meshgrove,
//! the folder's name and the path below it (`models/Garten/rose.stl`).

use std::path::{Component, Path, PathBuf};

use meshgrove_core::config::LibraryRead;
use meshgrove_core::{Kind, archive, listing};
use meshgrove_plugin_api::MeshData;
use serde_json::{Value, json};

use crate::library::{self, Caller, LibraryAccess, Located};

/// Most files a search looks at.
const MAX_FILES: usize = 100_000;

pub struct FolderLibrary {
    root: PathBuf,
    /// The folder's name, the first part of every id.
    alias: String,
}

impl FolderLibrary {
    pub fn new(root: &Path) -> std::io::Result<FolderLibrary> {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(std::io::Error::other(format!("{} is not a folder", root.display())));
        }
        let alias = match listing::file_name(&root) {
            name if name.is_empty() || name.contains(['/', '\\']) => "library".to_owned(),
            name => name,
        };
        Ok(FolderLibrary { root, alias })
    }

    /// The first part of every id.
    pub fn alias(&self) -> &str {
        &self.alias
    }

    fn id_of(&self, path: &Path) -> String {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        format!("{}/{}", self.alias, parts.join("/"))
    }

    /// The file behind an id; ids that leave the folder are unknown like any other.
    fn path_of(&self, id: &str) -> Result<PathBuf, String> {
        let unknown = || format!("unknown id: {id}");
        let rest = id.trim().trim_start_matches("meshgrove:");
        let rest = rest.strip_prefix(&self.alias).and_then(|r| r.strip_prefix('/')).ok_or_else(unknown)?;
        let rel = Path::new(rest);
        if rest.is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(unknown());
        }
        let path = rel.components().fold(self.root.clone(), |p, part| p.join(part));
        // A file, or one inside an archive.
        (path.is_file() || archive::split(&path).is_some()).then_some(path).ok_or_else(unknown)
    }

    /// The model files below the folder, by path.
    fn models(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut folders = vec![self.root.clone()];
        while let Some(dir) = folders.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    folders.push(path);
                } else if Kind::from_file_name(&listing::file_name(&path)).is_model() && found.len() < MAX_FILES {
                    found.push(path);
                }
            }
        }
        found.sort();
        found
    }

    fn allowed(read: LibraryRead) -> Result<(), String> {
        match read {
            LibraryRead::None => Err("not allowed to read the library".into()),
            _ => Ok(()),
        }
    }
}

impl LibraryAccess for FolderLibrary {
    /// The models whose id holds every word of `query` (`ext:stl` asks for a kind of file).
    fn search(&self, query: &str, limit: u32, read: LibraryRead, _caller: Caller) -> Result<Value, String> {
        FolderLibrary::allowed(read)?;
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let items: Vec<Value> = self
            .models()
            .iter()
            .filter_map(|path| {
                let (id, name) = (self.id_of(path), listing::file_name(path));
                let kind = Kind::from_file_name(&name).as_str();
                let lower = id.to_lowercase();
                let fits = words.iter().all(|w| match w.strip_prefix("ext:") {
                    Some(ext) => ext == kind,
                    None => lower.contains(w.as_str()),
                });
                let file_size = path.metadata().map_or(0, |m| m.len());
                fits.then(|| json!({"id": id, "name": name, "kind": kind, "file_size": file_size}))
            })
            .take(limit.max(1) as usize)
            .collect();
        Ok(json!({"items": items}))
    }

    fn describe(&self, id: &str, read: LibraryRead, _caller: Caller) -> Result<Value, String> {
        FolderLibrary::allowed(read)?;
        let path = self.path_of(id)?;
        let bytes = archive::read(&path).map_err(|e| e.to_string())?;
        let name = listing::file_name(&path);
        let mut facts = library::mesh_facts(&name, bytes.len(), &library::load_file(&path)?);
        facts["id"] = json!(self.id_of(&path));
        if Kind::from_file_name(&name) == Kind::ThreeMf {
            facts["parts"] = library::parts_json(&bytes, id.trim()).into();
        }
        Ok(facts)
    }

    fn triangles(
        &self,
        id: &str,
        part: Option<usize>,
        max_triangles: u32,
        read: LibraryRead,
        _caller: Caller,
    ) -> Result<MeshData, String> {
        FolderLibrary::allowed(read)?;
        let path = self.path_of(id)?;
        let mesh = match part {
            Some(part) => library::part_mesh(&archive::read(&path).map_err(|e| e.to_string())?, part)?,
            None => library::load_file(&path)?,
        };
        let data = library::simplify(&mesh, max_triangles as usize);
        if data.triangles() == 0 {
            return Err(format!("{id} has no triangles"));
        }
        Ok(data)
    }

    fn locate(&self, id: &str, read: LibraryRead, _caller: Caller) -> Result<Located, String> {
        FolderLibrary::allowed(read)?;
        Ok(Located { root: self.root.clone(), path: self.path_of(id)? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_reads_like_a_library() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("models");
        std::fs::create_dir_all(root.join("Garten")).unwrap();
        let stl = MeshData::cuboid([10.0, 20.0, 30.0]).to_stl("box");
        std::fs::write(root.join("Garten").join("Rose.stl"), &stl).unwrap();
        std::fs::write(root.join("box.stl"), &stl).unwrap();
        std::fs::write(root.join("notes.txt"), "no model").unwrap();
        std::fs::write(dir.path().join("secret.stl"), &stl).unwrap();
        let lib = FolderLibrary::new(&root).unwrap();
        let (all, user) = (LibraryRead::All, Caller::User);
        assert_eq!(lib.alias(), "models");

        let ids = |query: &str| -> Vec<String> {
            let found = lib.search(query, 50, all, user).unwrap();
            found["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_owned()).collect()
        };
        assert_eq!(ids(""), ["models/Garten/Rose.stl", "models/box.stl"], "models only");
        assert_eq!(ids("rose"), ["models/Garten/Rose.stl"]);
        assert_eq!(ids("garten ext:stl"), ["models/Garten/Rose.stl"]);
        assert!(ids("ext:3mf").is_empty() && ids("rose box").is_empty());
        assert_eq!(lib.search("", 1, all, user).unwrap()["items"].as_array().unwrap().len(), 1);

        let facts = lib.describe("models/Garten/Rose.stl", all, user).unwrap();
        assert_eq!((facts["id"].as_str(), facts["triangles"].as_u64()), (Some("models/Garten/Rose.stl"), Some(12)));
        assert_eq!(facts["size_mm"], json!([10.0, 20.0, 30.0]));
        assert_eq!(lib.triangles("models/box.stl", None, 1000, all, user).unwrap().triangles(), 12);
        let found = lib.locate("meshgrove:models/Garten/Rose.stl", all, user).unwrap();
        assert_eq!(
            (found.root, found.path),
            (root.canonicalize().unwrap(), root.canonicalize().unwrap().join("Garten").join("Rose.stl"))
        );

        for bad in [
            "models/../secret.stl",
            "models",
            "models/",
            "other/box.stl",
            "/etc/passwd",
            "models/Garten",
            "models/nope.stl",
        ] {
            assert!(lib.describe(bad, all, user).unwrap_err().contains("unknown id"), "{bad}");
            assert!(lib.locate(bad, all, user).is_err(), "{bad}");
        }
        assert!(lib.search("", 10, LibraryRead::None, user).is_err());
        assert!(FolderLibrary::new(&root.join("box.stl")).is_err() && FolderLibrary::new(&root.join("nope")).is_err());
    }
}
