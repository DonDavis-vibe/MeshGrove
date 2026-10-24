//! Applying accepted suggestions to the library, with a journal that takes them back: each
//! change is one row of `changes` holding the steps that were done and the steps that undo
//! exactly those, written in the same savepoint as the change itself.
//!
//! Steps work on content (hashes), like tags and notes do, so a file moved since the suggestion
//! is still found; a file whose content is gone or changed makes its step stale.

use std::path::PathBuf;
use std::time::SystemTime;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::ai::proposals::Target;
use crate::db::{Error, Library, nanos};
use crate::lists::ListKind;

/// One step of an accepted suggestion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ApplyOp {
    AddTag { target: Target, tag: String },
    RemoveTag { target: Target, tag: String },
    SetNote { target: Target, text: String, append: bool },
    /// A list (or the existing one of that kind and name) with these files and copies.
    List { kind: ListKind, name: String, query: Option<String>, items: Vec<(Target, u32)> },
    InboxDone { target: Target },
}

/// What takes a step back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "undo", rename_all = "snake_case")]
enum UndoOp {
    Untag { hash: String, tag: String },
    Retag { hash: String, tag: String },
    Note { hash: String, before: Option<String>, after: Option<String> },
    DeleteList { list: i64 },
    RemoveItem { list: i64, hash: String },
    Reopen { hash: String },
}

/// Where a suggestion's file is now.
#[derive(Debug, Clone, PartialEq)]
pub enum TargetState {
    Here,
    /// Same content at another place (moved or renamed since).
    Moved(PathBuf),
    /// Gone, or its content changed.
    Stale,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChangeReport {
    /// The journal row (0 when nothing was done).
    pub id: i64,
    pub applied: usize,
    /// Steps that did nothing, and why ("stale", "already there" …).
    pub skipped: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UndoReport {
    pub undone: usize,
    /// Steps not taken back because the user changed the same thing since.
    pub kept: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeInfo {
    pub id: i64,
    pub origin: String,
    pub title: String,
    pub applied_ns: i64,
    pub undone_ns: Option<i64>,
    /// Steps that can be undone.
    pub steps: usize,
}

impl Library {
    /// Finds a suggestion's file again.
    pub fn resolve_target(&self, t: &Target) -> Result<TargetState, Error> {
        let Some(hash) = &t.hash else {
            // Not processed when suggested: the same file, if size and time still match.
            let row = self.ai_row_of(&t.path)?;
            return Ok(match row {
                Some(r) if r.entry.size == t.size && nanos(r.entry.modified) == t.mtime_ns => TargetState::Here,
                _ => TargetState::Stale,
            });
        };
        let paths = self.paths_with_hash(hash)?;
        Ok(if paths.contains(&t.path) {
            TargetState::Here
        } else {
            match paths.into_iter().next() {
                Some(p) => TargetState::Moved(p),
                None => TargetState::Stale,
            }
        })
    }

    fn paths_with_hash(&self, hash: &str) -> Result<Vec<PathBuf>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT d.path, f.name FROM files f JOIN dirs d ON d.id = f.dir_id WHERE f.content_hash = ?1 ORDER BY d.path, f.name",
        )?;
        stmt.query_map([hash], |r| Ok(std::path::Path::new(&r.get::<_, String>(0)?).join(r.get::<_, String>(1)?)))?
            .collect()
    }

    /// The content a step works on; `None` when the file is stale.
    fn target_hash(&mut self, t: &Target) -> Result<Option<String>, Error> {
        match self.resolve_target(t)? {
            TargetState::Stale => Ok(None),
            _ => match &t.hash {
                Some(h) => Ok(Some(h.clone())),
                None => self.ensure_content_hash(&t.path),
            },
        }
    }

    /// The index entry of a file (also an archive member).
    pub fn entry_of(&self, path: &std::path::Path) -> Result<Option<crate::listing::Entry>, Error> {
        Ok(self.ai_row_of(path)?.map(|r| r.entry))
    }

    /// How many files carry content `hash` (copies get a tag too).
    pub fn copies_of(&self, hash: &str) -> Result<usize, Error> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM files WHERE content_hash = ?1", [hash], |r| r.get::<_, i64>(0))? as usize)
    }

    /// Applies `ops` in one go and journals how to undo them; `origin` names where they came from
    /// (`proposal:<uid>`, `chat:<message>`).
    pub fn apply_change(&mut self, origin: &str, title: &str, ops: &[ApplyOp]) -> Result<ChangeReport, Error> {
        self.conn.execute_batch("SAVEPOINT apply_change")?;
        match self.apply_ops(ops) {
            Ok((undo, report)) => {
                let result = (|| {
                    if undo.is_empty() {
                        return Ok(ChangeReport { id: 0, ..report });
                    }
                    self.conn.execute(
                        "INSERT INTO changes (origin, title, applied_ns, undo, ops) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![origin, title, nanos(Some(SystemTime::now())), json(&undo)?, json(ops)?],
                    )?;
                    Ok(ChangeReport { id: self.conn.last_insert_rowid(), ..report })
                })();
                match result {
                    Ok(report) => {
                        self.conn.execute_batch("RELEASE apply_change")?;
                        Ok(report)
                    }
                    Err(e) => {
                        self.conn.execute_batch("ROLLBACK TO apply_change; RELEASE apply_change")?;
                        Err(e)
                    }
                }
            }
            Err(e) => {
                self.conn.execute_batch("ROLLBACK TO apply_change; RELEASE apply_change")?;
                Err(e)
            }
        }
    }

    fn apply_ops(&mut self, ops: &[ApplyOp]) -> Result<(Vec<UndoOp>, ChangeReport), Error> {
        let mut undo = Vec::new();
        let mut report = ChangeReport::default();
        let now = nanos(Some(SystemTime::now()));
        for op in ops {
            let done = match op {
                ApplyOp::AddTag { target, tag } => match self.target_hash(target)? {
                    Some(hash) => {
                        let tag = tag.trim();
                        self.conn.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [tag])?;
                        let id: i64 =
                            self.conn.query_row("SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE", [tag], |r| r.get(0))?;
                        let added = self.conn.execute(
                            "INSERT OR IGNORE INTO file_tags (content_hash, tag_id) VALUES (?1, ?2)",
                            params![hash, id],
                        )? > 0;
                        if added {
                            undo.push(UndoOp::Untag { hash, tag: tag.to_owned() });
                        } else {
                            self.drop_unused_tags()?; // a tag created just now for nothing
                        }
                        added
                    }
                    None => false,
                },
                ApplyOp::RemoveTag { target, tag } => match self.target_hash(target)? {
                    Some(hash) => {
                        let name: Option<String> = self
                            .conn
                            .query_row(
                                "SELECT t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
                                 WHERE ft.content_hash = ?1 AND t.name = ?2 COLLATE NOCASE",
                                params![hash, tag.trim()],
                                |r| r.get(0),
                            )
                            .optional()?;
                        match name {
                            Some(name) => {
                                self.untag_hash(&hash, &name)?;
                                undo.push(UndoOp::Retag { hash, tag: name });
                                true
                            }
                            None => false,
                        }
                    }
                    None => false,
                },
                ApplyOp::SetNote { target, text, append } => match self.target_hash(target)? {
                    Some(hash) => {
                        let before = self.note_of_hash(&hash)?;
                        let after = match (&before, append) {
                            (Some(b), true) => format!("{b}\n\n{}", text.trim()),
                            _ => text.trim().to_owned(),
                        };
                        let after = Some(after).filter(|a| !a.is_empty());
                        if after == before {
                            false
                        } else {
                            self.set_note_hash(&hash, after.as_deref())?;
                            undo.push(UndoOp::Note { hash, before, after });
                            true
                        }
                    }
                    None => false,
                },
                ApplyOp::List { kind, name, query, items } => {
                    let existing: Option<i64> = self
                        .conn
                        .query_row(
                            "SELECT id FROM lists WHERE kind = ?1 AND name = ?2 COLLATE NOCASE",
                            params![kind.as_str(), name.trim()],
                            |r| r.get(0),
                        )
                        .optional()?;
                    let list = match existing {
                        Some(id) => id,
                        None => {
                            let id = self.create_list(*kind, name)?;
                            if let Some(q) = query {
                                self.conn.execute("UPDATE lists SET query = ?2 WHERE id = ?1", params![id, q.trim()])?;
                            }
                            undo.push(UndoOp::DeleteList { list: id });
                            id
                        }
                    };
                    let mut any = existing.is_none();
                    for (target, quantity) in items {
                        let Some(hash) = self.target_hash(target)? else {
                            report.skipped += 1;
                            continue;
                        };
                        let added = self.conn.execute(
                            "INSERT OR IGNORE INTO list_items (list_id, content_hash, quantity, added_ns) VALUES (?1, ?2, ?3, ?4)",
                            params![list, hash, (*quantity).max(1), now],
                        )? > 0;
                        // Items of a list created here go with the list.
                        if added && existing.is_some() {
                            undo.push(UndoOp::RemoveItem { list, hash });
                        }
                        any |= added;
                    }
                    any
                }
                ApplyOp::InboxDone { target } => match self.target_hash(target)? {
                    Some(hash) => {
                        let done = self.conn.execute(
                            "UPDATE inbox SET done_ns = ?2, done_by = 'proposal' WHERE content_hash = ?1 AND done_ns IS NULL",
                            params![hash, now],
                        )? > 0;
                        if done {
                            undo.push(UndoOp::Reopen { hash });
                        }
                        done
                    }
                    None => false,
                },
            };
            if done {
                report.applied += 1;
            } else {
                report.skipped += 1;
            }
        }
        Ok((undo, report))
    }

    fn untag_hash(&mut self, hash: &str, tag: &str) -> Result<(), Error> {
        self.conn.execute(
            "DELETE FROM file_tags WHERE content_hash = ?1 AND tag_id = (SELECT id FROM tags WHERE name = ?2 COLLATE NOCASE)",
            params![hash, tag],
        )?;
        self.drop_unused_tags()
    }

    fn drop_unused_tags(&mut self) -> Result<(), Error> {
        self.conn.execute("DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM file_tags)", [])?;
        Ok(())
    }

    fn note_of_hash(&self, hash: &str) -> Result<Option<String>, Error> {
        self.conn.query_row("SELECT text FROM notes WHERE content_hash = ?1", [hash], |r| r.get(0)).optional()
    }

    fn set_note_hash(&mut self, hash: &str, text: Option<&str>) -> Result<(), Error> {
        match text {
            None => self.conn.execute("DELETE FROM notes WHERE content_hash = ?1", [hash])?,
            Some(text) => self.conn.execute(
                "INSERT INTO notes (content_hash, text, search, updated_ns) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(content_hash) DO UPDATE SET text = excluded.text, search = excluded.search,
                     updated_ns = excluded.updated_ns",
                params![hash, text, text.to_lowercase(), nanos(Some(SystemTime::now()))],
            )?,
        };
        Ok(())
    }

    /// Takes a change back, newest step first. Steps whose result the user changed since stay.
    pub fn undo_change(&mut self, id: i64) -> Result<UndoReport, Error> {
        let row: Option<(String, Option<i64>)> = self
            .conn
            .query_row("SELECT undo, undone_ns FROM changes WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((undo, None)) = row else { return Ok(UndoReport::default()) };
        let steps: Vec<UndoOp> = serde_json::from_str(&undo).unwrap_or_default();
        let mut report = UndoReport::default();
        self.conn.execute_batch("SAVEPOINT undo_change")?;
        let result = (|| {
            for step in steps.iter().rev() {
                if self.undo_step(step)? {
                    report.undone += 1;
                } else {
                    report.kept += 1;
                }
            }
            self.conn.execute(
                "UPDATE changes SET undone_ns = ?2 WHERE id = ?1",
                params![id, nanos(Some(SystemTime::now()))],
            )?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("RELEASE undo_change")?;
                Ok(report)
            }
            Err(e) => {
                self.conn.execute_batch("ROLLBACK TO undo_change; RELEASE undo_change")?;
                Err(e)
            }
        }
    }

    fn undo_step(&mut self, step: &UndoOp) -> Result<bool, Error> {
        Ok(match step {
            UndoOp::Untag { hash, tag } => {
                let had = self.conn.query_row(
                    "SELECT COUNT(*) FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
                     WHERE ft.content_hash = ?1 AND t.name = ?2 COLLATE NOCASE",
                    params![hash, tag],
                    |r| r.get::<_, i64>(0),
                )? > 0;
                if had {
                    self.untag_hash(hash, tag)?;
                }
                had
            }
            UndoOp::Retag { hash, tag } => {
                self.conn.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [tag])?;
                let id: i64 = self.conn.query_row("SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE", [tag], |r| r.get(0))?;
                self.conn.execute("INSERT OR IGNORE INTO file_tags (content_hash, tag_id) VALUES (?1, ?2)", params![hash, id])?
                    > 0
            }
            UndoOp::Note { hash, before, after } => {
                // Only if the note is still what the change wrote.
                if self.note_of_hash(hash)? == *after {
                    self.set_note_hash(hash, before.as_deref())?;
                    true
                } else {
                    false
                }
            }
            UndoOp::DeleteList { list } => self.conn.execute("DELETE FROM lists WHERE id = ?1", [list])? > 0,
            UndoOp::RemoveItem { list, hash } => self
                .conn
                .execute("DELETE FROM list_items WHERE list_id = ?1 AND content_hash = ?2", params![list, hash])?
                > 0,
            UndoOp::Reopen { hash } => {
                self.conn.execute(
                    "UPDATE inbox SET done_ns = NULL, done_by = NULL WHERE content_hash = ?1 AND done_by = 'proposal'",
                    [hash],
                )? > 0
            }
        })
    }

    /// Does an undone change again (its steps as they were accepted).
    pub fn redo_change(&mut self, id: i64) -> Result<ChangeReport, Error> {
        let row: Option<(String, String, String, Option<i64>)> = self
            .conn
            .query_row("SELECT origin, title, ops, undone_ns FROM changes WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()?;
        let Some((origin, title, ops, Some(_))) = row else { return Ok(ChangeReport::default()) };
        let ops: Vec<ApplyOp> = serde_json::from_str(&ops).unwrap_or_default();
        let report = self.apply_change(&origin, &title, &ops)?;
        // The redone change takes the old one's place in the history.
        self.conn.execute("DELETE FROM changes WHERE id = ?1", [id])?;
        Ok(report)
    }

    /// The newest changes.
    pub fn changes(&self, limit: usize) -> Result<Vec<ChangeInfo>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, origin, title, applied_ns, undone_ns, undo FROM changes ORDER BY applied_ns DESC, id DESC LIMIT ?1",
        )?;
        stmt.query_map([limit as i64], |r| {
            let undo: String = r.get(5)?;
            Ok(ChangeInfo {
                id: r.get(0)?,
                origin: r.get(1)?,
                title: r.get(2)?,
                applied_ns: r.get(3)?,
                undone_ns: r.get(4)?,
                steps: serde_json::from_str::<Vec<UndoOp>>(&undo).map(|u| u.len()).unwrap_or(0),
            })
        })?
        .collect()
    }

    /// The changes that came from `origin` (all of one chat message, say), oldest first.
    pub fn changes_by_origin(&self, origin: &str) -> Result<Vec<i64>, Error> {
        let mut stmt = self.conn.prepare("SELECT id FROM changes WHERE origin = ?1 ORDER BY applied_ns, id")?;
        stmt.query_map([origin], |r| r.get(0))?.collect()
    }
}

fn json<T: Serialize + ?Sized>(value: &T) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|e| Error::ToSqlConversionFailure(Box::new(e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::ThumbMeta;
    use crate::formats::Kind;
    use crate::listing::Entry;
    use std::path::Path;
    use std::time::Duration;

    fn library() -> Library {
        let mut lib = Library::open_in_memory().unwrap();
        let r = Path::new("/r");
        let files: Vec<Entry> = ["a.stl", "b.stl"]
            .iter()
            .map(|n| Entry {
                name: (*n).into(),
                path: r.join(n),
                kind: Kind::Stl,
                size: 10,
                modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5)),
            })
            .collect();
        lib.update_dir(r, r, 1, &files, &[]).unwrap();
        for (n, h) in [("a.stl", "ha"), ("b.stl", "hb")] {
            lib.store_meta(&r.join(n), "k", &ThumbMeta { content_hash: Some(h.into()), ..Default::default() }).unwrap();
        }
        lib
    }

    fn target(name: &str, hash: &str) -> Target {
        Target { path: Path::new("/r").join(name), hash: Some(hash.into()), size: 10, mtime_ns: 5_000_000_000 }
    }

    fn tags(lib: &Library, file: &str) -> Vec<String> {
        lib.tags_of(&[Path::new("/r").join(file)]).unwrap().into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn tags_are_added_and_only_those_are_taken_back() {
        let mut lib = library();
        lib.tag(&[PathBuf::from("/r/a.stl")], "Garten").unwrap();
        let ops = [
            ApplyOp::AddTag { target: target("a.stl", "ha"), tag: "garten".into() },
            ApplyOp::AddTag { target: target("a.stl", "ha"), tag: "Rose".into() },
            ApplyOp::RemoveTag { target: target("a.stl", "ha"), tag: "Garten".into() },
        ];
        let report = lib.apply_change("proposal:x", "Test", &ops).unwrap();
        assert_eq!((report.applied, report.skipped), (2, 1), "Garten was there already");
        assert_eq!(tags(&lib, "a.stl"), ["Rose"]);
        let undo = lib.undo_change(report.id).unwrap();
        assert_eq!(undo.undone, 2);
        assert_eq!(tags(&lib, "a.stl"), ["Garten"], "the tag that was there before stays");
        assert_eq!(lib.all_tags().unwrap(), [("Garten".to_owned(), 1)], "Rose is gone completely");
        assert_eq!(lib.undo_change(report.id).unwrap(), UndoReport::default(), "only once");
        let redo = lib.redo_change(report.id).unwrap();
        assert_eq!(redo.applied, 2);
        assert_eq!(tags(&lib, "a.stl"), ["Rose"]);
        assert_eq!(lib.changes(9).unwrap().len(), 1, "the redo replaced the undone entry");
    }

    #[test]
    fn notes_lists_and_the_inbox() {
        let mut lib = library();
        lib.set_note(&[PathBuf::from("/r/a.stl")], "alt").unwrap();
        lib.conn.execute("UPDATE inbox SET done_ns = NULL, done_by = NULL", []).unwrap();
        let ops = [
            ApplyOp::SetNote { target: target("a.stl", "ha"), text: "neu".into(), append: true },
            ApplyOp::List {
                kind: ListKind::Print,
                name: "Markt".into(),
                query: None,
                items: vec![(target("a.stl", "ha"), 3), (target("b.stl", "hb"), 1)],
            },
            ApplyOp::InboxDone { target: target("b.stl", "hb") },
        ];
        let report = lib.apply_change("chat:7", "Chat", &ops).unwrap();
        assert_eq!(report.applied, 3);
        assert_eq!(lib.notes_of(&[PathBuf::from("/r/a.stl")]).unwrap().0.as_deref(), Some("alt\n\nneu"));
        let markt = lib.lists().unwrap().into_iter().find(|l| l.name == "Markt").unwrap();
        assert_eq!(lib.list_entries(markt.id).unwrap().iter().map(|e| e.quantity).sum::<u32>(), 4);
        assert_eq!(lib.inbox_count().unwrap(), 1);
        assert_eq!(lib.changes_by_origin("chat:7").unwrap(), [report.id]);

        // The user edits the note afterwards: undo leaves it.
        lib.set_note(&[PathBuf::from("/r/a.stl")], "von Hand").unwrap();
        let undo = lib.undo_change(report.id).unwrap();
        assert_eq!((undo.undone, undo.kept), (2, 1));
        assert_eq!(lib.notes_of(&[PathBuf::from("/r/a.stl")]).unwrap().0.as_deref(), Some("von Hand"));
        assert!(lib.lists().unwrap().iter().all(|l| l.name != "Markt"), "the list made by the change is gone");
        assert_eq!(lib.inbox_count().unwrap(), 2, "back in the inbox");
    }

    #[test]
    fn moved_files_are_found_and_changed_ones_skipped() {
        let mut lib = library();
        let moved = Target { path: "/elsewhere/a.stl".into(), ..target("a.stl", "ha") };
        assert_eq!(lib.resolve_target(&moved).unwrap(), TargetState::Moved(PathBuf::from("/r/a.stl")));
        let gone = target("c.stl", "hc");
        assert_eq!(lib.resolve_target(&gone).unwrap(), TargetState::Stale);
        let unprocessed = Target { hash: None, ..target("b.stl", "") };
        assert_eq!(lib.resolve_target(&unprocessed).unwrap(), TargetState::Here);
        let changed = Target { hash: None, size: 11, ..target("b.stl", "") };
        assert_eq!(lib.resolve_target(&changed).unwrap(), TargetState::Stale);
        let report = lib
            .apply_change(
                "proposal:y",
                "Test",
                &[
                    ApplyOp::AddTag { target: moved, tag: "Gefunden".into() },
                    ApplyOp::AddTag { target: gone, tag: "Weg".into() },
                ],
            )
            .unwrap();
        assert_eq!((report.applied, report.skipped), (1, 1));
        assert_eq!(tags(&lib, "a.stl"), ["Gefunden"]);
        assert!(lib.all_tags().unwrap().iter().all(|(t, _)| t != "Weg"), "no empty tag left behind");
        assert_eq!(lib.copies_of("ha").unwrap(), 1);
    }
}
