//! Tags, notes, lists and the print log outside the index: a readable JSON file for
//! export/import, and rolling daily backups so the hand-made part of the library survives a
//! deleted or broken index.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::lists::{ListKind, PrintResult};

pub const FORMAT: &str = "meshgrove-user-data";
/// Exports made before the project was renamed.
const LEGACY_FORMAT: &str = "stl-browser-user-data";
/// 2 added lists, prints and tag colours; version 1 files read as data without them.
const VERSION: u32 = 2;
const BACKUP_PREFIX: &str = "user-data-";

/// Tags, notes and prints keyed by file content, plus the lists and the chosen tag colours
/// (see `Library::export_user_data`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UserData {
    pub format: String,
    pub version: u32,
    pub entries: Vec<UserEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lists: Vec<ListData>,
    /// Palette slot by tag name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tag_colors: BTreeMap<String, u8>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UserEntry {
    pub content_hash: String,
    /// Where files with this content were when exported (for people reading the file).
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prints: Vec<PrintRecord>,
}

/// One logged print. Times are seconds since the epoch.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PrintRecord {
    pub result: PrintResult,
    pub at: i64,
    /// The print list it was counted on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ListData {
    pub kind: ListKind,
    pub name: String,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub created: i64,
    pub items: Vec<ListItemData>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ListItemData {
    pub content_hash: String,
    #[serde(default = "one")]
    pub quantity: u32,
    #[serde(default)]
    pub done: u32,
    #[serde(default)]
    pub added: i64,
}

fn one() -> u32 {
    1
}

impl UserData {
    pub fn new(entries: Vec<UserEntry>) -> UserData {
        UserData { format: FORMAT.into(), version: VERSION, entries, lists: Vec::new(), tag_colors: BTreeMap::new() }
    }

    /// Nothing hand-made to keep.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.lists.is_empty() && self.tag_colors.is_empty()
    }

    /// Same data, whatever the format version.
    fn same_content(&self, other: &UserData) -> bool {
        self.entries == other.entries && self.lists == other.lists && self.tag_colors == other.tag_colors
    }
}

/// Writes `data` as pretty JSON, replacing `path` only once the new file is complete.
pub fn write(data: &UserData, path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(data).map_err(io::Error::other)?)?;
    std::fs::rename(&tmp, path)
}

pub fn read(path: &Path) -> io::Result<UserData> {
    let data: UserData =
        serde_json::from_slice(&std::fs::read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if data.format != FORMAT && data.format != LEGACY_FORMAT {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a Meshgrove export"));
    }
    Ok(data)
}

/// `~/.local/share/meshgrove/backups`
pub fn backup_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("meshgrove").join("backups"))
}

/// Writes today's backup unless there is nothing to keep or the newest backup already holds the
/// same data, then deletes all but the newest `keep` backups. Returns the file written.
pub fn rolling_backup(data: &UserData, dir: &Path, today: &str, keep: usize) -> io::Result<Option<PathBuf>> {
    let mut existing = backups(dir);
    let unchanged = existing.last().and_then(|p| read(p).ok()).is_some_and(|last| last.same_content(data));
    let mut written = None;
    if !data.is_empty() && !unchanged {
        let path = dir.join(format!("{BACKUP_PREFIX}{today}.json"));
        write(data, &path)?;
        if !existing.contains(&path) {
            existing.push(path.clone());
        }
        written = Some(path);
    }
    for old in existing.iter().take(existing.len().saturating_sub(keep)) {
        std::fs::remove_file(old)?;
    }
    Ok(written)
}

/// Backup files in `dir`, oldest first (the dated names sort chronologically).
pub fn backups(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(BACKUP_PREFIX) && n.ends_with(".json"))
        })
        .collect();
    files.sort();
    files
}

/// `YYYY-MM-DD` (UTC) of `time`.
pub fn date(time: SystemTime) -> String {
    let days = time.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64;
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn data(note: &str) -> UserData {
        UserData::new(vec![UserEntry {
            content_hash: "h".into(),
            files: vec!["/lib/orc.stl".into()],
            tags: vec!["bemalt".into()],
            note: Some(note.into()),
            prints: Vec::new(),
        }])
    }

    #[test]
    fn dates() {
        assert_eq!(date(SystemTime::UNIX_EPOCH), "1970-01-01");
        assert_eq!(date(SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_540_800)), "2026-10-21");
        assert_eq!(date(SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29");
    }

    #[test]
    fn write_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.json");
        write(&data("x"), &path).unwrap();
        assert_eq!(read(&path).unwrap(), data("x"));
        std::fs::write(&path, br#"{"format":"other","version":1,"entries":[]}"#).unwrap();
        assert!(read(&path).is_err());
        std::fs::write(&path, br#"{"format":"stl-browser-user-data","version":1,"entries":[]}"#).unwrap();
        assert!(read(&path).is_ok(), "exports from before the rename still import");
    }

    #[test]
    fn rolling_backups_skip_unchanged_and_keep_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(rolling_backup(&UserData::new(vec![]), d, "2026-10-01", 3).unwrap(), None, "nothing to keep");
        assert!(rolling_backup(&data("a"), d, "2026-10-01", 3).unwrap().is_some());
        assert_eq!(rolling_backup(&data("a"), d, "2026-10-02", 3).unwrap(), None, "unchanged");
        for (day, note) in [("02", "b"), ("03", "c"), ("04", "d")] {
            rolling_backup(&data(note), d, &format!("2026-10-{day}"), 3).unwrap();
        }
        let names: Vec<String> =
            backups(d).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["user-data-2026-10-02.json", "user-data-2026-10-03.json", "user-data-2026-10-04.json"]);
        // Same day again: the day's file is replaced, not duplicated.
        rolling_backup(&data("e"), d, "2026-10-04", 3).unwrap();
        assert_eq!(backups(d).len(), 3);
        assert_eq!(read(&d.join("user-data-2026-10-04.json")).unwrap(), data("e"));
    }
}
