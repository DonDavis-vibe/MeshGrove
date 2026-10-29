//! Suggestions from AI, waiting for the user: a database of its own (`proposals.db`) that
//! `meshgrove-mcp` and the endpoint worker write and Meshgrove reads. The library itself only
//! changes when the user accepts a suggestion in Meshgrove; deleting this file deletes every
//! trace of AI work (with the access log).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::vocab::{MAX_TAG_CHARS, TagStatus, Vocabulary, valid_tag};
use super::{Context, ToolError, ToolResult};
use crate::config::AccessClass;
use crate::db::nanos;

const STORE_VERSION: i32 = 1;
/// Most suggestions in one batch.
pub const MAX_ITEMS: usize = 500;
/// Most undecided batches one client may leave waiting.
pub const MAX_PENDING: usize = 50;
/// Most tags added to one file at once.
pub const MAX_TAGS_PER_FILE: usize = 8;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS batches (
    id INTEGER PRIMARY KEY,
    uid TEXT NOT NULL UNIQUE,
    -- 'mcp', 'endpoint' or 'chat'
    source TEXT NOT NULL,
    client TEXT,
    access_class TEXT NOT NULL,
    -- 'tags', 'note', 'list', 'moves' or 'mixed'
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT,
    -- accepting it also takes its files out of the inbox
    inbox_done INTEGER NOT NULL DEFAULT 0,
    created_ns INTEGER NOT NULL,
    -- 'pending', 'partly', 'accepted', 'rejected', 'undone' or 'expired'
    state TEXT NOT NULL DEFAULT 'pending',
    decided_ns INTEGER
);
CREATE TABLE IF NOT EXISTS proposals (
    id INTEGER PRIMARY KEY,
    batch_id INTEGER NOT NULL REFERENCES batches(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    -- JSON Target, NULL for suggestions that are not about one file (a new list)
    target TEXT,
    payload TEXT NOT NULL,
    reason TEXT,
    confidence REAL,
    -- 'pending', 'accepted', 'rejected' or 'stale'
    state TEXT NOT NULL DEFAULT 'pending',
    decided_ns INTEGER
);
CREATE INDEX IF NOT EXISTS proposals_batch ON proposals(batch_id);
CREATE TABLE IF NOT EXISTS chats (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    created_ns INTEGER NOT NULL,
    updated_ns INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS chat_messages (
    id INTEGER PRIMARY KEY,
    chat_id INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    -- 'user' or 'assistant'
    role TEXT NOT NULL,
    -- JSON: text, references, tool calls, cards (what the chat view shows)
    content TEXT NOT NULL,
    created_ns INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS chat_messages_chat ON chat_messages(chat_id);
CREATE TABLE IF NOT EXISTS access_log (
    id INTEGER PRIMARY KEY,
    at_ns INTEGER NOT NULL,
    source TEXT NOT NULL,
    client TEXT,
    access_class TEXT NOT NULL,
    tool TEXT NOT NULL,
    items INTEGER NOT NULL,
    images INTEGER NOT NULL DEFAULT 0,
    -- aliases of the libraries the answer came from
    libraries TEXT
);
";

/// The file a suggestion is about, as the index knew it then. Meshgrove finds it again by
/// content when it was moved, and calls the suggestion stale when it changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    pub size: u64,
    pub mtime_ns: i64,
}

/// Who suggests.
#[derive(Debug, Clone, PartialEq)]
pub struct Origin {
    /// "mcp", "endpoint" or "chat".
    pub source: String,
    /// The client's name ("claude-desktop", "lm-studio" …) or the endpoint's model.
    pub client: Option<String>,
    pub class: AccessClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    Tags,
    Note,
    List,
    Move,
}

impl ProposalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalKind::Tags => "tags",
            ProposalKind::Note => "note",
            ProposalKind::List => "list",
            ProposalKind::Move => "move",
        }
    }

    fn parse(s: &str) -> ProposalKind {
        match s {
            "note" => ProposalKind::Note,
            "list" => ProposalKind::List,
            "move" => ProposalKind::Move,
            _ => ProposalKind::Tags,
        }
    }
}

/// Tags to add (spelled as they will be stored, with their status) and to remove.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagsPayload {
    pub add: Vec<TagOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagOut {
    pub name: String,
    pub status: TagStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotePayload {
    pub text: String,
    /// The note replaces the old one, or is added below it.
    pub append: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListPayload {
    pub name: String,
    /// "print" or "collection".
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ListItemPayload>,
    /// An automatic collection: this saved search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Where a file, archive or folder goes, in the library it is in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MovePayload {
    pub root: PathBuf,
    pub to: PathBuf,
    pub folder: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItemPayload {
    pub target: Target,
    pub quantity: u32,
}

pub struct NewProposal {
    pub kind: ProposalKind,
    pub target: Option<Target>,
    pub payload: serde_json::Value,
    pub reason: Option<String>,
    pub confidence: Option<f32>,
}

pub struct NewBatch<'a> {
    pub origin: &'a Origin,
    pub kind: &'a str,
    pub title: &'a str,
    pub summary: Option<&'a str>,
    pub inbox_done: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BatchInfo {
    pub id: i64,
    pub uid: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub access_class: String,
    pub kind: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub inbox_done: bool,
    pub created_ns: i64,
    pub state: String,
    pub items: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub id: i64,
    pub batch_id: i64,
    pub kind: ProposalKind,
    pub target: Option<Target>,
    pub payload: serde_json::Value,
    pub reason: Option<String>,
    pub confidence: Option<f32>,
    pub state: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accepted,
    Rejected,
    /// The file changed or vanished: nothing could be done.
    Stale,
}

impl Decision {
    fn as_str(self) -> &'static str {
        match self {
            Decision::Accepted => "accepted",
            Decision::Rejected => "rejected",
            Decision::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccessEntry {
    pub at_ns: i64,
    pub source: String,
    pub client: Option<String>,
    pub access_class: String,
    pub tool: String,
    pub items: usize,
    pub images: usize,
    pub libraries: Vec<String>,
}

pub struct ProposalStore {
    conn: Connection,
}

impl ProposalStore {
    /// `~/.local/share/meshgrove/proposals.db`
    pub fn default_location() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("meshgrove").join("proposals.db"))
    }

    pub fn open(path: &Path) -> ToolResult<ProposalStore> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> ToolResult<ProposalStore> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> ToolResult<ProposalStore> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > STORE_VERSION {
            return Err(ToolError::StoreNewer(version));
        }
        conn.execute_batch(SCHEMA)?;
        if version < STORE_VERSION {
            conn.pragma_update(None, "user_version", STORE_VERSION)?;
        }
        Ok(ProposalStore { conn })
    }

    /// Changes whenever another connection wrote (cheap; for noticing new suggestions).
    pub fn data_version(&self) -> ToolResult<i64> {
        Ok(self.conn.query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }

    pub fn insert_batch(&mut self, batch: &NewBatch, items: &[NewProposal]) -> ToolResult<(i64, String)> {
        let uid = new_uid();
        let now = nanos(Some(SystemTime::now()));
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO batches (uid, source, client, access_class, kind, title, summary, inbox_done, created_ns)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                uid,
                batch.origin.source,
                batch.origin.client,
                batch.origin.class.as_str(),
                batch.kind,
                batch.title,
                batch.summary,
                batch.inbox_done,
                now
            ],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut insert = tx.prepare(
                "INSERT INTO proposals (batch_id, kind, target, payload, reason, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for p in items {
                let target = p.target.as_ref().map(|t| serde_json::to_string(t).unwrap_or_default());
                insert.execute(params![id, p.kind.as_str(), target, p.payload.to_string(), p.reason, p.confidence])?;
            }
        }
        tx.commit()?;
        Ok((id, uid))
    }

    /// Batches waiting for a decision.
    pub fn pending_count(&self) -> ToolResult<usize> {
        let n: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM batches WHERE state IN ('pending', 'partly')", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    pub(crate) fn pending_of(&self, origin: &Origin) -> ToolResult<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM batches WHERE state = 'pending' AND source = ?1 AND client IS ?2",
            params![origin.source, origin.client],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// The files whose move `source` suggested and which still waits.
    pub fn pending_move_sources(&self, source: &str) -> ToolResult<std::collections::HashSet<PathBuf>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.target FROM proposals p JOIN batches b ON b.id = p.batch_id
             WHERE b.source = ?1 AND p.kind = 'move' AND p.state = 'pending'",
        )?;
        let rows = stmt.query_map([source], |r| r.get::<_, Option<String>>(0))?;
        let mut out = std::collections::HashSet::new();
        for row in rows {
            if let Some(t) = row?.and_then(|t| serde_json::from_str::<Target>(&t).ok()) {
                out.insert(t.path);
            }
        }
        Ok(out)
    }

    /// Newest first; only those of `source` (and `client`) when given, only `states` when given.
    pub fn batches(
        &self,
        source: Option<&str>,
        client: Option<&str>,
        states: &[&str],
        limit: usize,
    ) -> ToolResult<Vec<BatchInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT b.id, b.uid, b.source, b.client, b.access_class, b.kind, b.title, b.summary, b.inbox_done,
                    b.created_ns, b.state, (SELECT COUNT(*) FROM proposals p WHERE p.batch_id = b.id)
             FROM batches b WHERE (?1 IS NULL OR b.source = ?1) AND (?2 IS NULL OR b.client = ?2)
             ORDER BY b.created_ns DESC, b.id DESC",
        )?;
        let rows = stmt.query_map(params![source, client], |r| {
            Ok(BatchInfo {
                id: r.get(0)?,
                uid: r.get(1)?,
                source: r.get(2)?,
                client: r.get(3)?,
                access_class: r.get(4)?,
                kind: r.get(5)?,
                title: r.get(6)?,
                summary: r.get(7)?,
                inbox_done: r.get(8)?,
                created_ns: r.get(9)?,
                state: r.get(10)?,
                items: r.get::<_, i64>(11)? as usize,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            let b = row?;
            if states.is_empty() || states.contains(&b.state.as_str()) {
                out.push(b);
                if out.len() == limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// A batch's state ('pending', 'partly', 'accepted' …), by its uid.
    pub fn batch_state(&self, uid: &str) -> ToolResult<Option<String>> {
        Ok(self.conn.query_row("SELECT state FROM batches WHERE uid = ?1", [uid], |r| r.get(0)).optional()?)
    }

    pub fn batch_by_uid(&self, uid: &str) -> ToolResult<Option<i64>> {
        Ok(self.conn.query_row("SELECT id FROM batches WHERE uid = ?1", [uid], |r| r.get(0)).optional()?)
    }

    pub fn proposals(&self, batch_id: i64) -> ToolResult<Vec<Proposal>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, batch_id, kind, target, payload, reason, confidence, state FROM proposals
             WHERE batch_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([batch_id], |r| {
            let target: Option<String> = r.get(3)?;
            let payload: String = r.get(4)?;
            Ok(Proposal {
                id: r.get(0)?,
                batch_id: r.get(1)?,
                kind: ProposalKind::parse(&r.get::<_, String>(2)?),
                target: target.and_then(|t| serde_json::from_str(&t).ok()),
                payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
                reason: r.get(5)?,
                confidence: r.get::<_, Option<f64>>(6)?.map(|c| c as f32),
                state: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records the user's decisions; the batch is accepted, rejected or partly done after them.
    pub fn decide(&mut self, batch_id: i64, decisions: &[(i64, Decision)]) -> ToolResult<()> {
        let now = nanos(Some(SystemTime::now()));
        let tx = self.conn.transaction()?;
        for (id, d) in decisions {
            tx.execute(
                "UPDATE proposals SET state = ?3, decided_ns = ?4 WHERE id = ?1 AND batch_id = ?2",
                params![id, batch_id, d.as_str(), now],
            )?;
        }
        let (pending, accepted): (i64, i64) = tx.query_row(
            "SELECT SUM(state = 'pending'), SUM(state = 'accepted') FROM proposals WHERE batch_id = ?1",
            [batch_id],
            |r| Ok((r.get::<_, Option<i64>>(0)?.unwrap_or(0), r.get::<_, Option<i64>>(1)?.unwrap_or(0))),
        )?;
        let state = match (pending, accepted) {
            (0, 0) => "rejected",
            (0, _) => "accepted",
            _ => "partly",
        };
        tx.execute("UPDATE batches SET state = ?2, decided_ns = ?3 WHERE id = ?1", params![batch_id, state, now])?;
        tx.commit()?;
        Ok(())
    }

    /// Sets a batch's state without deciding its suggestions one by one (after a crash, the
    /// library's journal shows it was applied).
    pub fn set_batch_state(&mut self, batch_id: i64, state: &str) -> ToolResult<()> {
        let now = nanos(Some(SystemTime::now()));
        self.conn
            .execute("UPDATE batches SET state = ?2, decided_ns = ?3 WHERE id = ?1", params![batch_id, state, now])?;
        Ok(())
    }

    /// An accepted batch was taken back again.
    pub fn mark_undone(&mut self, batch_id: i64) -> ToolResult<()> {
        let now = nanos(Some(SystemTime::now()));
        self.conn
            .execute("UPDATE batches SET state = 'undone', decided_ns = ?2 WHERE id = ?1", params![batch_id, now])?;
        Ok(())
    }

    pub fn log_access(&mut self, e: &AccessEntry) -> ToolResult<()> {
        self.conn.execute(
            "INSERT INTO access_log (at_ns, source, client, access_class, tool, items, images, libraries)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                e.at_ns,
                e.source,
                e.client,
                e.access_class,
                e.tool,
                e.items as i64,
                e.images as i64,
                e.libraries.join(", ")
            ],
        )?;
        Ok(())
    }

    /// The newest `n` accesses.
    pub fn access_log(&self, n: usize) -> ToolResult<Vec<AccessEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT at_ns, source, client, access_class, tool, items, images, libraries FROM access_log
             ORDER BY at_ns DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([n as i64], |r| {
            let libraries: Option<String> = r.get(7)?;
            Ok(AccessEntry {
                at_ns: r.get(0)?,
                source: r.get(1)?,
                client: r.get(2)?,
                access_class: r.get(3)?,
                tool: r.get(4)?,
                items: r.get::<_, i64>(5)? as usize,
                images: r.get::<_, i64>(6)? as usize,
                libraries: libraries
                    .filter(|l| !l.is_empty())
                    .map(|l| l.split(", ").map(str::to_owned).collect())
                    .unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Forgets old log lines and lets old undecided suggestions expire.
    pub fn prune(&mut self, now: SystemTime, log_days: u16, expire_days: u16) -> ToolResult<()> {
        let day = Duration::from_secs(86_400);
        let before = |days: u16| nanos(now.checked_sub(day * u32::from(days)));
        self.conn.execute("DELETE FROM access_log WHERE at_ns < ?1", [before(log_days)])?;
        self.conn.execute(
            "UPDATE batches SET state = 'expired', decided_ns = ?2 WHERE state = 'pending' AND created_ns < ?1",
            params![before(expire_days), nanos(Some(now))],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatInfo {
    pub id: i64,
    pub title: String,
    pub updated_ns: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredMessage {
    pub id: i64,
    pub role: String,
    pub content: serde_json::Value,
    pub created_ns: i64,
}

impl ProposalStore {
    pub fn create_chat(&mut self, title: &str) -> ToolResult<i64> {
        let now = nanos(Some(SystemTime::now()));
        self.conn
            .execute("INSERT INTO chats (title, created_ns, updated_ns) VALUES (?1, ?2, ?2)", params![title, now])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_chat(&mut self, id: i64, title: &str) -> ToolResult<()> {
        self.conn.execute("UPDATE chats SET title = ?2 WHERE id = ?1", params![id, title])?;
        Ok(())
    }

    pub fn delete_chat(&mut self, id: i64) -> ToolResult<()> {
        self.conn.execute("DELETE FROM chats WHERE id = ?1", [id])?;
        Ok(())
    }

    /// The newest conversations first.
    pub fn chats(&self, limit: usize) -> ToolResult<Vec<ChatInfo>> {
        let mut stmt =
            self.conn.prepare("SELECT id, title, updated_ns FROM chats ORDER BY updated_ns DESC, id DESC LIMIT ?1")?;
        let rows = stmt
            .query_map([limit as i64], |r| Ok(ChatInfo { id: r.get(0)?, title: r.get(1)?, updated_ns: r.get(2)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn messages(&self, chat: i64) -> ToolResult<Vec<StoredMessage>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, role, content, created_ns FROM chat_messages WHERE chat_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([chat], |r| {
            let content: String = r.get(2)?;
            Ok(StoredMessage {
                id: r.get(0)?,
                role: r.get(1)?,
                content: serde_json::from_str(&content).unwrap_or(serde_json::Value::Null),
                created_ns: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn add_message(&mut self, chat: i64, role: &str, content: &serde_json::Value) -> ToolResult<i64> {
        let now = nanos(Some(SystemTime::now()));
        self.conn.execute(
            "INSERT INTO chat_messages (chat_id, role, content, created_ns) VALUES (?1, ?2, ?3, ?4)",
            params![chat, role, content.to_string(), now],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute("UPDATE chats SET updated_ns = ?2 WHERE id = ?1", params![chat, now])?;
        Ok(id)
    }

    pub fn update_message(&mut self, id: i64, content: &serde_json::Value) -> ToolResult<()> {
        self.conn.execute("UPDATE chat_messages SET content = ?2 WHERE id = ?1", params![id, content.to_string()])?;
        Ok(())
    }

    /// Forgets conversations not touched for `days`.
    pub fn prune_chats(&mut self, now: SystemTime, days: u16) -> ToolResult<()> {
        let before = nanos(now.checked_sub(Duration::from_secs(86_400) * u32::from(days)));
        self.conn.execute("DELETE FROM chats WHERE updated_ns < ?1", [before])?;
        Ok(())
    }
}

/// A short id nobody else uses: blake3 of the time, the process and a counter.
fn new_uid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = format!(
        "{}:{}:{}",
        nanos(Some(SystemTime::now())),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    blake3::hash(seed.as_bytes()).to_hex()[..16].to_owned()
}

// ---- From suggestions to changes ----

/// What the user picked of one suggestion in the review.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Picked {
    /// Tags to add and to remove, as named in the suggestion.
    pub add: Vec<String>,
    pub remove: Vec<String>,
    /// The note, the list (or the move) as suggested.
    pub whole: bool,
}

impl Picked {
    /// Everything a suggestion offers.
    pub fn all(p: &Proposal) -> Picked {
        match p.kind {
            ProposalKind::Tags => {
                let tags: Option<TagsPayload> = serde_json::from_value(p.payload.clone()).ok();
                let tags = tags.unwrap_or(TagsPayload { add: Vec::new(), remove: Vec::new() });
                Picked { add: tags.add.into_iter().map(|t| t.name).collect(), remove: tags.remove, whole: false }
            }
            _ => Picked { whole: true, ..Picked::default() },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty() && !self.whole
    }
}

/// The library changes for what was picked of `p`; `inbox_done` also takes its file out of the inbox.
pub fn to_ops(p: &Proposal, picked: &Picked, inbox_done: bool) -> Vec<crate::changes::ApplyOp> {
    use crate::changes::ApplyOp;
    use crate::lists::ListKind;
    let mut ops = Vec::new();
    match (p.kind, &p.target) {
        (ProposalKind::Tags, Some(t)) => {
            ops.extend(picked.add.iter().map(|tag| ApplyOp::AddTag { target: t.clone(), tag: tag.clone() }));
            ops.extend(picked.remove.iter().map(|tag| ApplyOp::RemoveTag { target: t.clone(), tag: tag.clone() }));
        }
        (ProposalKind::Note, Some(t)) if picked.whole => {
            if let Ok(note) = serde_json::from_value::<NotePayload>(p.payload.clone()) {
                ops.push(ApplyOp::SetNote { target: t.clone(), text: note.text, append: note.append });
            }
        }
        (ProposalKind::Move, Some(t)) if picked.whole => {
            if let Ok(m) = serde_json::from_value::<MovePayload>(p.payload.clone()) {
                ops.push(ApplyOp::Move { root: m.root, target: t.clone(), to: m.to, folder: m.folder });
            }
        }
        (ProposalKind::List, _) if picked.whole => {
            if let Ok(list) = serde_json::from_value::<ListPayload>(p.payload.clone()) {
                let kind = if list.kind == "print" { ListKind::Print } else { ListKind::Collection };
                let items = list.items.into_iter().map(|i| (i.target, i.quantity)).collect();
                ops.push(ApplyOp::List { kind, name: list.name, query: list.query, items });
            }
        }
        _ => {}
    }
    if inbox_done
        && !ops.is_empty()
        && let Some(t) = &p.target
    {
        ops.push(ApplyOp::InboxDone { target: t.clone() });
    }
    ops
}

// ---- The suggesting tools ----

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TagItem {
    /// A file id (from search_models, list_inbox …).
    pub id: String,
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
    /// Why, in a few words (shown in the review).
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProposeTags {
    /// Shown above the suggestions, e.g. "Kunde Müller einordnen".
    pub title: String,
    pub items: Vec<TagItem>,
    /// Accepting the suggestions also takes the files out of the inbox.
    #[serde(default)]
    pub mark_inbox_done: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NoteItem {
    pub id: String,
    pub note: String,
    /// Add below the existing note instead of replacing it.
    #[serde(default)]
    pub append: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProposeNote {
    #[serde(default)]
    pub title: Option<String>,
    pub items: Vec<NoteItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListItemIn {
    pub id: String,
    /// Copies to print (print lists); default 1.
    #[serde(default)]
    pub quantity: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProposeList {
    pub name: String,
    /// "print" (with quantities, printed log) or "collection".
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub items: Vec<ListItemIn>,
    /// Instead of items: a saved search in Meshgrove syntax, making an automatic collection.
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MoveItem {
    /// A file, archive or folder (an id from search_models, folder_summary …).
    pub id: String,
    /// Its new id in the same library, e.g. "Garten/Rankhilfen/rose.stl" (only the name changes
    /// for a rename); ending in "/" the folder it goes into, keeping its name. Missing folders
    /// are made.
    pub to: String,
    /// Why, in a few words (shown in the review).
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProposeMoves {
    /// Shown above the suggestions, e.g. "Kunde Müller aufräumen".
    pub title: String,
    pub items: Vec<MoveItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProposeTagMerge {
    /// Tags to replace (spelling variants, synonyms, the other language).
    pub from: Vec<String>,
    /// The tag they become.
    pub to: String,
}

/// What a suggesting tool answers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Receipt {
    pub batch: String,
    pub items: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<RejectedItem>,
    /// The tags as they will be stored, and whether they are new.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<TagOut>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RejectedItem {
    pub id: String,
    pub why: String,
}

const WAITING: &str = "Nothing has changed yet: the suggestions wait in Meshgrove until the user accepts them.";
const ACCEPTED: &str = "The user lets Meshgrove accept suggestions by itself: these are applied once Meshgrove runs, \
and can be undone in its history.";

const ACCEPTED_LANDING: &str = "The user lets Meshgrove accept suggestions by itself, and these moves take files \
out of the import folder: they are applied once Meshgrove runs, and can be undone in its history.";

/// What happens to new suggestions: they wait for the review, or (when the user turned that on)
/// Meshgrove accepts them by itself; not the chat's, which has its own switch, and moves only out
/// of the import folder ([`next_step_moves`]).
fn next_step(cx: &Context, origin: &Origin) -> &'static str {
    if cx.config.ai.auto_accept_since.is_some() && origin.source != "chat" { ACCEPTED } else { WAITING }
}

/// Moves wait for the review, unless every one of them takes a file out of the import folder
/// and the user lets Meshgrove accept by itself.
fn next_step_moves(cx: &Context, origin: &Origin, proposals: &[NewProposal]) -> &'static str {
    let landing = proposals.iter().all(|p| p.target.as_ref().is_some_and(|t| cx.config.in_landing(&t.path)));
    if landing && next_step(cx, origin) == ACCEPTED { ACCEPTED_LANDING } else { WAITING }
}

fn target(cx: &Context, id: &str) -> ToolResult<Target> {
    let path = cx.resolve(id)?;
    let row = cx.lib.ai_row_of(&path)?.ok_or_else(|| ToolError::UnknownId(id.to_owned()))?;
    Ok(Target { path, hash: row.hash, size: row.entry.size, mtime_ns: nanos(row.entry.modified) })
}

fn check_room(store: &ProposalStore, origin: &Origin, items: usize) -> ToolResult<()> {
    if items > MAX_ITEMS {
        return Err(ToolError::Invalid(format!("at most {MAX_ITEMS} items per call: split the work into batches")));
    }
    if store.pending_of(origin)? >= MAX_PENDING {
        return Err(ToolError::Invalid(format!(
            "{MAX_PENDING} batches already wait for the user: ask them to review in Meshgrove first"
        )));
    }
    Ok(())
}

fn vocabulary(cx: &Context) -> ToolResult<Vocabulary> {
    Ok(Vocabulary::new(cx.config, cx.lib.all_tags()?.into_iter().map(|(t, _)| t).collect()))
}

fn summary_line(text: Option<&str>) -> Option<String> {
    text.map(|t| super::untrusted(t.trim(), 200)).filter(|t| !t.is_empty())
}

pub fn propose_tags(
    cx: &Context,
    store: &mut ProposalStore,
    origin: &Origin,
    req: &ProposeTags,
) -> ToolResult<Receipt> {
    check_room(store, origin, req.items.len())?;
    let vocab = vocabulary(cx)?;
    let mut proposals = Vec::new();
    let mut rejected = Vec::new();
    let mut tags: Vec<TagOut> = Vec::new();
    for item in &req.items {
        let t = match target(cx, &item.id) {
            Ok(t) => t,
            Err(e) => {
                rejected.push(RejectedItem { id: item.id.clone(), why: e.to_string() });
                continue;
            }
        };
        if let Some(bad) = item.add.iter().chain(&item.remove).find(|t| !valid_tag(t)) {
            let why = format!("tag {bad:?}: one line, at most {MAX_TAG_CHARS} characters");
            rejected.push(RejectedItem { id: item.id.clone(), why });
            continue;
        }
        if item.add.len() > MAX_TAGS_PER_FILE {
            rejected
                .push(RejectedItem { id: item.id.clone(), why: format!("at most {MAX_TAGS_PER_FILE} tags per file") });
            continue;
        }
        let mut add: Vec<TagOut> = Vec::new();
        for tag in &item.add {
            let (name, status) = vocab.normalize(tag);
            if !add.iter().any(|a| a.name.eq_ignore_ascii_case(&name)) {
                add.push(TagOut { name, status });
            }
        }
        let remove: Vec<String> = item.remove.iter().map(|t| vocab.normalize(t).0).collect();
        if add.is_empty() && remove.is_empty() {
            rejected.push(RejectedItem { id: item.id.clone(), why: "nothing to add or remove".into() });
            continue;
        }
        for a in &add {
            if !tags.iter().any(|t| t.name == a.name) {
                tags.push(a.clone());
            }
        }
        proposals.push(NewProposal {
            kind: ProposalKind::Tags,
            target: Some(t),
            payload: serde_json::to_value(TagsPayload { add, remove }).unwrap_or_default(),
            reason: summary_line(item.reason.as_deref()),
            confidence: None,
        });
    }
    if proposals.is_empty() {
        return Err(ToolError::Invalid(format!(
            "no usable suggestion: {}",
            rejected.iter().map(|r| format!("{} ({})", r.id, r.why)).collect::<Vec<_>>().join("; ")
        )));
    }
    let title = summary_line(Some(&req.title)).unwrap_or_else(|| "Tags".into());
    let batch = NewBatch { origin, kind: "tags", title: &title, summary: None, inbox_done: req.mark_inbox_done };
    let (_, uid) = store.insert_batch(&batch, &proposals)?;
    let new = tags.iter().filter(|t| t.status == TagStatus::New).count();
    let step = next_step(cx, origin);
    let message = if new > 0 {
        format!("{step} {new} of the tags are new to the library; the review marks them.")
    } else {
        step.to_owned()
    };
    Ok(Receipt { batch: uid, items: proposals.len(), rejected, tags, message })
}

pub fn propose_note(
    cx: &Context,
    store: &mut ProposalStore,
    origin: &Origin,
    req: &ProposeNote,
) -> ToolResult<Receipt> {
    check_room(store, origin, req.items.len())?;
    let mut proposals = Vec::new();
    let mut rejected = Vec::new();
    for item in &req.items {
        match target(cx, &item.id) {
            Ok(t) if !item.note.trim().is_empty() => proposals.push(NewProposal {
                kind: ProposalKind::Note,
                target: Some(t),
                payload: serde_json::to_value(NotePayload {
                    text: super::untrusted(item.note.trim(), 4000),
                    append: item.append,
                })
                .unwrap_or_default(),
                reason: None,
                confidence: None,
            }),
            Ok(_) => rejected.push(RejectedItem { id: item.id.clone(), why: "empty note".into() }),
            Err(e) => rejected.push(RejectedItem { id: item.id.clone(), why: e.to_string() }),
        }
    }
    if proposals.is_empty() {
        return Err(ToolError::Invalid("no usable note".into()));
    }
    let title = summary_line(req.title.as_deref()).unwrap_or_else(|| "Notizen".into());
    let batch = NewBatch { origin, kind: "note", title: &title, summary: None, inbox_done: false };
    let (_, uid) = store.insert_batch(&batch, &proposals)?;
    Ok(Receipt {
        batch: uid,
        items: proposals.len(),
        rejected,
        tags: Vec::new(),
        message: next_step(cx, origin).into(),
    })
}

pub fn propose_list(
    cx: &Context,
    store: &mut ProposalStore,
    origin: &Origin,
    req: &ProposeList,
) -> ToolResult<Receipt> {
    check_room(store, origin, req.items.len())?;
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(ToolError::Invalid("a list needs a name of one line, at most 80 characters".into()));
    }
    let kind = match req.kind.as_deref().map(str::trim) {
        None | Some("print") => "print",
        Some("collection") => "collection",
        Some(other) => return Err(ToolError::Invalid(format!("unknown list kind {other:?}: print or collection"))),
    };
    let query = req.query.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(str::to_owned);
    if query.is_none() && req.items.is_empty() {
        return Err(ToolError::Invalid("give items or a query".into()));
    }
    let mut items = Vec::new();
    let mut rejected = Vec::new();
    for item in &req.items {
        match target(cx, &item.id) {
            Ok(t) => items.push(ListItemPayload { target: t, quantity: item.quantity.unwrap_or(1).clamp(1, 999) }),
            Err(e) => rejected.push(RejectedItem { id: item.id.clone(), why: e.to_string() }),
        }
    }
    if query.is_none() && items.is_empty() {
        return Err(ToolError::Invalid("none of the items is known".into()));
    }
    let count = items.len();
    let payload = ListPayload {
        name: name.to_owned(),
        kind: if query.is_some() { "collection".into() } else { kind.into() },
        items,
        query,
        description: summary_line(req.description.as_deref()),
    };
    let proposal = NewProposal {
        kind: ProposalKind::List,
        target: None,
        payload: serde_json::to_value(&payload).unwrap_or_default(),
        reason: payload.description.clone(),
        confidence: None,
    };
    let batch = NewBatch { origin, kind: "list", title: name, summary: None, inbox_done: false };
    let (_, uid) = store.insert_batch(&batch, &[proposal])?;
    Ok(Receipt { batch: uid, items: count, rejected, tags: Vec::new(), message: next_step(cx, origin).into() })
}

/// Replaces tags by one: per visible file carrying one of `from`, a suggestion to remove it
/// and add `to`.
/// Moves and renames, checked now and again when accepted: within one library, never onto
/// anything, nothing inside an archive, names that work on every system.
pub fn propose_moves(
    cx: &Context,
    store: &mut ProposalStore,
    origin: &Origin,
    req: &ProposeMoves,
) -> ToolResult<Receipt> {
    check_room(store, origin, req.items.len())?;
    let mut proposals = Vec::new();
    let mut rejected = Vec::new();
    let (mut targets, mut sources): (Vec<PathBuf>, Vec<PathBuf>) = (Vec::new(), Vec::new());
    for item in &req.items {
        match move_of(cx, item, &targets, &sources) {
            Ok((target, payload)) => {
                targets.push(payload.to.clone());
                sources.push(target.path.clone());
                proposals.push(NewProposal {
                    kind: ProposalKind::Move,
                    target: Some(target),
                    payload: serde_json::to_value(payload).unwrap_or_default(),
                    reason: summary_line(item.reason.as_deref()),
                    confidence: None,
                });
            }
            Err(why) => rejected.push(RejectedItem { id: item.id.clone(), why }),
        }
    }
    if proposals.is_empty() {
        let why = rejected.first().map_or("no items", |r| r.why.as_str());
        return Err(ToolError::Invalid(format!("no usable move ({why})")));
    }
    let title = summary_line(Some(&req.title)).unwrap_or_else(|| "Verschieben".into());
    let message = next_step_moves(cx, origin, &proposals).to_owned();
    let batch = NewBatch { origin, kind: "move", title: &title, summary: None, inbox_done: false };
    let (_, uid) = store.insert_batch(&batch, &proposals)?;
    Ok(Receipt { batch: uid, items: proposals.len(), rejected, tags: Vec::new(), message })
}

/// One move, checked; `targets` and `sources` are those of the batch so far.
fn move_of(
    cx: &Context,
    item: &MoveItem,
    targets: &[PathBuf],
    sources: &[PathBuf],
) -> Result<(Target, MovePayload), String> {
    let from = cx.resolve(&item.id).map_err(|e| e.to_string())?;
    let root = cx.scope.root_of(&from).map(|r| r.path.clone()).ok_or_else(|| format!("unknown id: {}", item.id))?;
    let to = match item.to.trim().strip_suffix('/') {
        Some(dir) => cx.resolve(dir).map(|d| d.join(from.file_name().unwrap_or_default())),
        None => cx.resolve(item.to.trim()),
    }
    .map_err(|_| format!("the new place {:?} is no id in a library you may see", item.to))?;
    if cx.scope.root_of(&to).map(|r| r.path.as_path()) != Some(root.as_path()) {
        return Err("moves stay within one library".into());
    }
    let (target, payload) = crate::moves::plan_move(cx.lib, &root, &from, &to)?;
    let lower = |p: &Path| p.to_string_lossy().to_lowercase();
    if targets.iter().any(|t| lower(t) == lower(&to)) {
        return Err("another move of this batch goes there".into());
    }
    if sources.iter().any(|s| from.starts_with(s) || s.starts_with(&from)) {
        return Err("it moves with a folder of this batch already".into());
    }
    Ok((target, payload))
}

pub fn propose_tag_merge(
    cx: &Context,
    store: &mut ProposalStore,
    origin: &Origin,
    req: &ProposeTagMerge,
) -> ToolResult<Receipt> {
    if !valid_tag(&req.to) || req.from.is_empty() {
        return Err(ToolError::Invalid("give the tags to replace and a valid tag they become".into()));
    }
    let mut items: Vec<TagItem> = Vec::new();
    for from in &req.from {
        let filter = super::types::ModelFilter {
            tags_all: vec![from.clone()],
            kinds: ["stl", "3mf", "step", "obj", "ply", "gltf", "gcode", "image", "pdf", "text"]
                .map(str::to_owned)
                .to_vec(),
            limit: Some(200),
            ..Default::default()
        };
        let mut cursor = None;
        loop {
            let page = super::read::search_models(
                cx,
                &super::types::ModelFilter { cursor: cursor.clone(), ..filter.clone() },
            )?;
            for m in page.items {
                match items.iter_mut().find(|i| i.id == m.id) {
                    Some(i) => i.remove.push(from.clone()),
                    None => items.push(TagItem {
                        id: m.id,
                        add: vec![req.to.clone()],
                        remove: vec![from.clone()],
                        reason: None,
                    }),
                }
            }
            match page.next_cursor {
                Some(next) if items.len() < MAX_ITEMS => cursor = Some(next),
                _ => break,
            }
        }
    }
    if items.is_empty() {
        return Err(ToolError::Invalid("no visible file carries these tags".into()));
    }
    let title = format!("{} → {}", req.from.join(", "), req.to);
    propose_tags(cx, store, origin, &ProposeTags { title, items, mark_inbox_done: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> Origin {
        Origin { source: "mcp".into(), client: Some("test".into()), class: AccessClass::Cloud }
    }

    #[test]
    fn batches_are_stored_decided_and_expired() {
        let mut store = ProposalStore::open_in_memory().unwrap();
        let o = origin();
        let item = |n: &str| NewProposal {
            kind: ProposalKind::Tags,
            target: Some(Target { path: n.into(), hash: Some("h".into()), size: 1, mtime_ns: 2 }),
            payload: serde_json::json!({"add": [{"name": "Garten", "status": "existing"}]}),
            reason: Some("Rosen".into()),
            confidence: None,
        };
        let batch = NewBatch { origin: &o, kind: "tags", title: "Eingang", summary: None, inbox_done: true };
        let (id, uid) = store.insert_batch(&batch, &[item("/a.stl"), item("/b.stl")]).unwrap();
        assert_eq!(uid.len(), 16);
        assert_eq!(store.pending_count().unwrap(), 1);
        let ps = store.proposals(id).unwrap();
        assert_eq!(ps[0].target.as_ref().unwrap().path, Path::new("/a.stl"));
        store.decide(id, &[(ps[0].id, Decision::Accepted)]).unwrap();
        assert_eq!(store.batches(None, None, &[], 9).unwrap()[0].state, "partly");
        store.decide(id, &[(ps[1].id, Decision::Rejected)]).unwrap();
        assert_eq!(store.batches(None, None, &[], 9).unwrap()[0].state, "accepted");
        assert_eq!(store.pending_count().unwrap(), 0);

        let (old, _) = store.insert_batch(&batch, &[item("/c.stl")]).unwrap();
        store.conn.execute("UPDATE batches SET created_ns = 1 WHERE id = ?1", [old]).unwrap();
        store.prune(SystemTime::now(), 90, 30).unwrap();
        assert_eq!(store.batches(None, None, &["expired"], 9).unwrap().len(), 1);
    }

    #[test]
    fn a_store_of_a_newer_version_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proposals.db");
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(ProposalStore::open(&path), Err(ToolError::StoreNewer(99))));
    }

    #[test]
    fn conversations_are_kept() {
        let mut store = ProposalStore::open_in_memory().unwrap();
        let chat = store.create_chat("Gartenmarkt").unwrap();
        let first = store.add_message(chat, "user", &serde_json::json!({"text": "Hallo"})).unwrap();
        store.add_message(chat, "assistant", &serde_json::json!({"text": "Hi"})).unwrap();
        store.update_message(first, &serde_json::json!({"text": "Hallo!", "changes": [3]})).unwrap();
        let messages = store.messages(chat).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content["changes"][0], 3);
        assert_eq!(store.chats(9).unwrap()[0].title, "Gartenmarkt");
        store.prune_chats(SystemTime::now() + Duration::from_secs(86_400 * 100), 90).unwrap();
        assert!(store.chats(9).unwrap().is_empty());
        assert!(store.messages(chat).unwrap().is_empty(), "messages go with their chat");
    }

    #[test]
    fn the_access_log_keeps_what_was_read() {
        let mut store = ProposalStore::open_in_memory().unwrap();
        let e = AccessEntry {
            at_ns: nanos(Some(SystemTime::now())),
            source: "mcp".into(),
            client: None,
            access_class: "cloud".into(),
            tool: "search_models".into(),
            items: 14,
            images: 0,
            libraries: vec!["Garten".into(), "Werkstatt".into()],
        };
        store.log_access(&e).unwrap();
        assert_eq!(store.access_log(5).unwrap(), [e]);
    }
}
