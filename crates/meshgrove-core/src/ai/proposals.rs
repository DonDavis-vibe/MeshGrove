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
use crate::config::{AccessClass, Config};
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
    -- 'mcp', 'endpoint', 'chat' or 'plugin:<id>'
    source TEXT NOT NULL,
    client TEXT,
    access_class TEXT NOT NULL,
    -- 'tags', 'note', 'list', 'moves', 'bundle' or 'mixed'
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
CREATE TABLE IF NOT EXISTS questions (
    id INTEGER PRIMARY KEY,
    uid TEXT NOT NULL UNIQUE,
    source TEXT NOT NULL,
    client TEXT,
    created_ns INTEGER NOT NULL,
    -- JSON AskUser: what the AI asks
    payload TEXT NOT NULL,
    -- JSON: what the user answered, NULL while they have not
    answer TEXT,
    answered_ns INTEGER
);
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
    /// "mcp", "endpoint", "chat" or "plugin:<id>".
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
    /// Files a plugin made, with their tags and lists ([`crate::bundle::ResolvedBundle`]).
    Bundle,
    /// A file, archive or folder to the trash. Always waits for the review; the trash, not the
    /// history, brings it back.
    Delete,
    /// An archive unpacked beside itself, files packed into a new ZIP, or added to one
    /// ([`crate::archives::ArchivePayload`]). Always waits for the review.
    Archive,
}

impl ProposalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalKind::Tags => "tags",
            ProposalKind::Note => "note",
            ProposalKind::List => "list",
            ProposalKind::Move => "move",
            ProposalKind::Bundle => "bundle",
            ProposalKind::Delete => "delete",
            ProposalKind::Archive => "archive",
        }
    }

    fn parse(s: &str) -> ProposalKind {
        match s {
            "note" => ProposalKind::Note,
            "list" => ProposalKind::List,
            "move" => ProposalKind::Move,
            "bundle" => ProposalKind::Bundle,
            "delete" => ProposalKind::Delete,
            "archive" => ProposalKind::Archive,
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
    /// Other places that fit as well (placed by tag, with several tags): the review offers them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItemPayload {
    pub target: Target,
    pub quantity: u32,
}

/// What goes to the trash: the library it is in, and whether it is a folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeletePayload {
    pub root: PathBuf,
    pub folder: bool,
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

    /// The file the store is in; `None` for one in memory.
    pub fn path(&self) -> Option<PathBuf> {
        self.conn.path().filter(|p| !p.is_empty()).map(PathBuf::from)
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
    /// Whether a suggestion still waiting names `text` (a path as JSON writes it).
    pub fn waiting_mentions(&self, text: &str) -> ToolResult<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM proposals WHERE state = 'pending' AND instr(payload, ?1) > 0)",
            [text],
            |r| r.get(0),
        )?)
    }

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
        self.conn.execute("DELETE FROM questions WHERE created_ns < ?1", [before(1)])?;
        Ok(())
    }

    /// Stores a question of the AI; its uid.
    pub fn ask(&mut self, origin: &Origin, ask: &AskUser) -> ToolResult<String> {
        let uid = new_uid();
        let payload = serde_json::to_value(ask).map_err(|e| ToolError::Invalid(e.to_string()))?;
        self.conn.execute(
            "INSERT INTO questions (uid, source, client, created_ns, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![uid, origin.source, origin.client, nanos(Some(SystemTime::now())), payload.to_string()],
        )?;
        Ok(uid)
    }

    /// The questions nobody answered yet, oldest first (`source`, `client`: only those).
    pub fn pending_questions(&self, source: Option<&str>, client: Option<&str>) -> ToolResult<Vec<PendingQuestion>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uid, source, client, created_ns, payload FROM questions
             WHERE answer IS NULL AND (?1 IS NULL OR source = ?1) AND (?2 IS NULL OR client = ?2) ORDER BY id",
        )?;
        let rows = stmt.query_map(params![source, client], |r| {
            let payload: String = r.get(5)?;
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, payload))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, uid, source, client, created_ns, payload) = row?;
            if let Ok(ask) = serde_json::from_str::<AskUser>(&payload) {
                out.push(PendingQuestion { id, uid, source, client, created_ns, ask });
            }
        }
        Ok(out)
    }

    /// The user's answer to a question, once there is one.
    pub fn answer_of(&self, uid: &str) -> ToolResult<Option<serde_json::Value>> {
        let text: Option<Option<String>> =
            self.conn.query_row("SELECT answer FROM questions WHERE uid = ?1", [uid], |r| r.get(0)).optional()?;
        Ok(text.flatten().and_then(|t| serde_json::from_str(&t).ok()))
    }

    /// Records the user's answer (or that they did not answer: `{"cancelled": true}`).
    pub fn answer(&mut self, uid: &str, answer: &serde_json::Value) -> ToolResult<()> {
        self.conn.execute(
            "UPDATE questions SET answer = ?2, answered_ns = ?3 WHERE uid = ?1 AND answer IS NULL",
            params![uid, answer.to_string(), nanos(Some(SystemTime::now()))],
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

/// What the user picked of one suggestion in the review, and what they changed of it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Picked {
    /// Tags to add and to remove: those of the suggestion that were kept, and any the user
    /// typed in the review.
    pub add: Vec<String>,
    pub remove: Vec<String>,
    /// The note, the list, the move (or the deletion), as suggested or as changed below.
    pub whole: bool,
    /// A move: the place chosen in the review instead of the suggested one.
    pub to: Option<PathBuf>,
    /// A note: the text as the user rewrote it, and whether it is added below the old one.
    pub note: Option<String>,
    pub append: Option<bool>,
    /// A list: its name and kind ("print" or "collection") as changed, and the items kept, by
    /// their index in the suggestion with the copies wanted.
    pub list_name: Option<String>,
    pub list_kind: Option<String>,
    pub list_items: Option<Vec<(usize, u32)>>,
}

impl Picked {
    /// Everything a suggestion offers.
    pub fn all(p: &Proposal) -> Picked {
        match p.kind {
            ProposalKind::Tags => {
                let tags: Option<TagsPayload> = serde_json::from_value(p.payload.clone()).ok();
                let tags = tags.unwrap_or(TagsPayload { add: Vec::new(), remove: Vec::new() });
                Picked { add: tags.add.into_iter().map(|t| t.name).collect(), remove: tags.remove, ..Picked::default() }
            }
            _ => Picked { whole: true, ..Picked::default() },
        }
    }

    /// Everything a suggestion offers, as the user changed it in the review ("Accept all"): what
    /// they unticked comes back, what they typed or rewrote stays (own tags, the note, the place,
    /// the list's name, kind and copies).
    pub fn everything(&self, p: &Proposal) -> Picked {
        let mut all = Picked::all(p);
        for tag in &self.add {
            if !all.add.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                all.add.push(tag.clone());
            }
        }
        // A tag typed in the review that the suggestion would take away is kept.
        all.remove.retain(|t| !self.add.iter().any(|a| a.eq_ignore_ascii_case(t)));
        all.to = self.to.clone();
        all.note = self.note.clone();
        all.append = self.append;
        all.list_name = self.list_name.clone();
        all.list_kind = self.list_kind.clone();
        if let Some(kept) = &self.list_items
            && let Ok(list) = serde_json::from_value::<ListPayload>(p.payload.clone())
        {
            let copies = |i: usize, suggested: u32| kept.iter().find(|(k, _)| *k == i).map_or(suggested, |(_, n)| *n);
            all.list_items =
                Some(list.items.iter().enumerate().map(|(i, item)| (i, copies(i, item.quantity))).collect());
        }
        all
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
                let text = picked.note.clone().unwrap_or(note.text);
                let append = picked.append.unwrap_or(note.append);
                if !text.trim().is_empty() {
                    ops.push(ApplyOp::SetNote { target: t.clone(), text, append });
                }
            }
        }
        (ProposalKind::Move, Some(t)) if picked.whole => {
            if let Ok(m) = serde_json::from_value::<MovePayload>(p.payload.clone()) {
                let to = picked.to.clone().unwrap_or(m.to);
                ops.push(ApplyOp::Move { root: m.root, target: t.clone(), to, folder: m.folder });
            }
        }
        (ProposalKind::List, _) if picked.whole => {
            if let Ok(list) = serde_json::from_value::<ListPayload>(p.payload.clone()) {
                let kind = picked.list_kind.clone().unwrap_or(list.kind);
                let kind = if kind == "print" { ListKind::Print } else { ListKind::Collection };
                let name = picked
                    .list_name
                    .clone()
                    .map(|n| n.trim().to_owned())
                    .filter(|n| !n.is_empty())
                    .unwrap_or(list.name);
                let items: Vec<(Target, u32)> = match &picked.list_items {
                    Some(kept) => kept
                        .iter()
                        .filter_map(|&(i, n)| list.items.get(i).map(|item| (item.target.clone(), n.clamp(1, 999))))
                        .collect(),
                    None => list.items.into_iter().map(|i| (i.target, i.quantity)).collect(),
                };
                if list.query.is_some() || !items.is_empty() {
                    ops.push(ApplyOp::List { kind, name, query: list.query, items });
                }
            }
        }
        (ProposalKind::Bundle, _) if picked.whole => {
            if let Ok(bundle) = serde_json::from_value::<crate::bundle::ResolvedBundle>(p.payload.clone()) {
                // Its place may have been taken while it waited.
                ops.extend(bundle.at_free_place().ops());
            }
        }
        (ProposalKind::Archive, Some(t)) if picked.whole => {
            if let Ok(payload) = serde_json::from_value::<crate::archives::ArchivePayload>(p.payload.clone()) {
                ops.extend(crate::archives::to_ops(t, &payload));
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeleteItem {
    /// A file, archive or folder (an id from search_models, folder_summary …).
    pub id: String,
    /// Why, in a few words (shown in the review).
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProposeDelete {
    /// Shown above the suggestions, e.g. "Doppelte Downloads entfernen".
    pub title: String,
    pub items: Vec<DeleteItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ArchiveItem {
    /// "unpack": the archive into a new folder named after it, beside it (the archive stays);
    /// "pack": files and folders into a new ZIP beside the first of them; "add": files and
    /// folders into a ZIP that is there.
    pub op: String,
    /// unpack, add: the archive (an id from search_models, folder_summary, list_archive …).
    #[serde(default)]
    pub archive: Option<String>,
    /// pack, add: the files and folders (ids) that go in, with what lies below the folders.
    #[serde(default)]
    pub ids: Vec<String>,
    /// pack: the new ZIP's name, e.g. "Rosen.zip" (a taken name gets a number).
    #[serde(default)]
    pub name: Option<String>,
    /// add: the folder inside the archive the files go below, e.g. "minis"; none: the top.
    #[serde(default)]
    pub folder: Option<String>,
    /// Why, in a few words (shown in the review).
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProposeArchive {
    /// Shown above the suggestions, e.g. "Download-Packs entpacken".
    pub title: String,
    pub items: Vec<ArchiveItem>,
}

/// One question to the user, answered by picking among options (or in their own words).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AskQuestion {
    pub question: String,
    /// The answers to pick from; none: the user answers in their own words.
    #[serde(default)]
    pub options: Vec<String>,
    /// Several options may be picked (checkboxes) instead of one (radio buttons).
    #[serde(default)]
    pub multi: bool,
    /// Besides the options, the user may write an answer of their own.
    #[serde(default)]
    pub allow_other: bool,
}

/// Questions to the user, shown in Meshgrove's chat with radio buttons or checkboxes; the call
/// returns once the user answered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AskUser {
    /// One to five questions.
    pub questions: Vec<AskQuestion>,
}

/// Most questions in one call, options per question, and characters per text.
pub const MAX_QUESTIONS: usize = 5;
pub const MAX_OPTIONS: usize = 12;
const MAX_QUESTION_CHARS: usize = 400;
const MAX_OPTION_CHARS: usize = 120;

impl AskUser {
    /// The questions as asked, trimmed and bounded; what is wrong with them otherwise.
    pub fn checked(&self) -> Result<AskUser, String> {
        if self.questions.is_empty() || self.questions.len() > MAX_QUESTIONS {
            return Err(format!("ask one to {MAX_QUESTIONS} questions"));
        }
        let mut questions = Vec::new();
        for q in &self.questions {
            let question = super::untrusted(q.question.trim(), MAX_QUESTION_CHARS);
            if question.is_empty() {
                return Err("a question is empty".into());
            }
            if q.options.len() > MAX_OPTIONS {
                return Err(format!("at most {MAX_OPTIONS} options per question"));
            }
            let mut options: Vec<String> = Vec::new();
            for o in &q.options {
                let o = super::untrusted(o.trim(), MAX_OPTION_CHARS);
                if !o.is_empty() && !options.iter().any(|x| x == &o) {
                    options.push(o);
                }
            }
            let allow_other = q.allow_other || options.is_empty();
            questions.push(AskQuestion { question, options, multi: q.multi, allow_other });
        }
        Ok(AskUser { questions })
    }
}

/// What `ask_user` answers at once: the question's id; the answer comes when the user gave it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AskReceipt {
    pub question: String,
    pub message: String,
}

/// A question waiting in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingQuestion {
    pub id: i64,
    pub uid: String,
    pub source: String,
    pub client: Option<String>,
    pub created_ns: i64,
    pub ask: AskUser,
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
        let remove: Vec<String> = item.remove.iter().map(|t| vocab.normalize(t).0).collect();
        let mut add: Vec<TagOut> = Vec::new();
        for tag in &item.add {
            let (mut name, mut status) = vocab.normalize(tag);
            // "Drone" would become the "Drohne" in use; with "Drohne" going at the same time it
            // is a rename, and the new spelling is meant.
            if remove.iter().any(|r| r.eq_ignore_ascii_case(&name)) {
                (name, status) = vocab.as_written(tag);
            }
            if !add.iter().any(|a| a.name.eq_ignore_ascii_case(&name))
                && !remove.iter().any(|r| r.eq_ignore_ascii_case(&name))
            {
                add.push(TagOut { name, status });
            }
        }
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

/// Files a plugin made, checked and placed ([`crate::bundle::resolve`]): one suggestion,
/// accepted by itself only where the user lets this plugin's be.
pub fn propose_bundle(
    config: &Config,
    store: &mut ProposalStore,
    origin: &Origin,
    bundle: &crate::bundle::ResolvedBundle,
) -> ToolResult<Receipt> {
    check_room(store, origin, 1)?;
    let proposal = NewProposal {
        kind: ProposalKind::Bundle,
        target: None,
        payload: serde_json::to_value(bundle).map_err(|e| ToolError::Invalid(e.to_string()))?,
        reason: bundle.summary.clone(),
        confidence: None,
    };
    let summary = bundle.summary.as_deref();
    let batch = NewBatch { origin, kind: "bundle", title: &bundle.title, summary, inbox_done: false };
    let (_, uid) = store.insert_batch(&batch, &[proposal])?;
    let auto = config.ai.auto_accept_since.is_some()
        && origin.source != "chat"
        && !bundle.review
        && config.plugin(&bundle.plugin).is_some_and(|p| p.grants.auto_accept);
    let message = if auto { ACCEPTED } else { WAITING };
    Ok(Receipt {
        batch: uid,
        items: bundle.files.len(),
        rejected: Vec::new(),
        tags: Vec::new(),
        message: message.into(),
    })
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
    // Into another library only when the user allows it; the move then belongs to that one.
    let to_root = cx.scope.root_of(&to).map(|r| r.path.clone()).ok_or("the new place is in no library you may see")?;
    let root = if to_root == root {
        root
    } else if cx.config.ai.cross_library_moves {
        to_root
    } else {
        return Err(
            "moves stay within one library (the user can allow moves between libraries in Settings → AI)".into()
        );
    };
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

/// Files, archives or folders for the trash: always reviewed, never inside an archive, never a
/// library itself.
pub fn propose_delete(
    cx: &Context,
    store: &mut ProposalStore,
    origin: &Origin,
    req: &ProposeDelete,
) -> ToolResult<Receipt> {
    check_room(store, origin, req.items.len())?;
    let mut proposals = Vec::new();
    let mut rejected = Vec::new();
    for item in &req.items {
        match delete_of(cx, item) {
            Ok((target, payload)) => proposals.push(NewProposal {
                kind: ProposalKind::Delete,
                target: Some(target),
                payload: serde_json::to_value(payload).unwrap_or_default(),
                reason: summary_line(item.reason.as_deref()),
                confidence: None,
            }),
            Err(why) => rejected.push(RejectedItem { id: item.id.clone(), why }),
        }
    }
    if proposals.is_empty() {
        let why = rejected.first().map_or("no items", |r| r.why.as_str());
        return Err(ToolError::Invalid(format!("no usable deletion ({why})")));
    }
    let title = summary_line(Some(&req.title)).unwrap_or_else(|| "Löschen".into());
    let batch = NewBatch { origin, kind: "delete", title: &title, summary: None, inbox_done: false };
    let (_, uid) = store.insert_batch(&batch, &proposals)?;
    Ok(Receipt {
        batch: uid,
        items: proposals.len(),
        rejected,
        tags: Vec::new(),
        message: "Nothing has changed yet: deletions always wait for the user's review in Meshgrove, which puts the \
                  files into the system's trash."
            .into(),
    })
}

fn delete_of(cx: &Context, item: &DeleteItem) -> Result<(Target, DeletePayload), String> {
    let path = cx.resolve(&item.id).map_err(|e| e.to_string())?;
    let root = cx.scope.root_of(&path).map(|r| r.path.clone()).ok_or_else(|| format!("unknown id: {}", item.id))?;
    if path == root {
        return Err("a library itself cannot be deleted".into());
    }
    if crate::archive::split(&path).is_some() {
        return Err("nothing inside an archive can be deleted; delete the archive".into());
    }
    let meta = path.symlink_metadata().map_err(|_| format!("{} is gone", item.id))?;
    let folder = meta.is_dir();
    let target = if folder {
        Target { path, hash: None, size: 0, mtime_ns: nanos(meta.modified().ok()) }
    } else {
        target(cx, &item.id).map_err(|e| e.to_string())?
    };
    Ok((target, DeletePayload { root, folder }))
}

/// Archives unpacked beside themselves, files packed into a new ZIP or added to one: always
/// reviewed, never replacing anything; only ZIP is written.
pub fn propose_archive(
    cx: &Context,
    store: &mut ProposalStore,
    origin: &Origin,
    req: &ProposeArchive,
) -> ToolResult<Receipt> {
    check_room(store, origin, req.items.len())?;
    let mut proposals = Vec::new();
    let mut rejected = Vec::new();
    for item in &req.items {
        match archive_of(cx, item) {
            Ok((target, payload)) => proposals.push(NewProposal {
                kind: ProposalKind::Archive,
                target: Some(target),
                payload: serde_json::to_value(payload).unwrap_or_default(),
                reason: summary_line(item.reason.as_deref()),
                confidence: None,
            }),
            Err(why) => {
                let id = item.archive.clone().or_else(|| item.ids.first().cloned()).unwrap_or_else(|| item.op.clone());
                rejected.push(RejectedItem { id, why });
            }
        }
    }
    if proposals.is_empty() {
        let why = rejected.first().map_or("no items", |r| r.why.as_str());
        return Err(ToolError::Invalid(format!("no usable archive operation ({why})")));
    }
    let title = summary_line(Some(&req.title)).unwrap_or_else(|| "Archive".into());
    let batch = NewBatch { origin, kind: "archive", title: &title, summary: None, inbox_done: false };
    let (_, uid) = store.insert_batch(&batch, &proposals)?;
    Ok(Receipt {
        batch: uid,
        items: proposals.len(),
        rejected,
        tags: Vec::new(),
        message: "Nothing has changed yet: archive operations always wait for the user's review in Meshgrove. \
                  Nothing is ever replaced, and the history takes them back."
            .into(),
    })
}

fn archive_of(cx: &Context, item: &ArchiveItem) -> Result<(Target, crate::archives::ArchivePayload), String> {
    use crate::archives::{plan_add, plan_pack, plan_unpack};
    let resolve = |id: &str| cx.resolve(id).map_err(|e| e.to_string());
    let root_of = |path: &Path| {
        cx.scope.root_of(path).map(|r| r.path.clone()).ok_or_else(|| format!("{} is not in a library", path.display()))
    };
    let planned = match item.op.as_str() {
        "unpack" => {
            let archive = resolve(item.archive.as_deref().ok_or("unpack needs the archive's id")?)?;
            plan_unpack(&root_of(&archive)?, &archive)
        }
        "pack" => {
            let sources = item.ids.iter().map(|id| resolve(id)).collect::<Result<Vec<_>, _>>()?;
            let first = sources.first().ok_or("pack needs the ids of what goes in")?;
            let name = item.name.as_deref().ok_or("pack needs a name for the ZIP")?;
            plan_pack(&root_of(first)?, &sources, name)
        }
        "add" => {
            let archive = resolve(item.archive.as_deref().ok_or("add needs the archive's id")?)?;
            let sources = item.ids.iter().map(|id| resolve(id)).collect::<Result<Vec<_>, _>>()?;
            let folder = item.folder.as_deref().unwrap_or("");
            plan_add(&root_of(&archive)?, &archive, folder, &sources)
        }
        other => return Err(format!("unknown op {other:?}: unpack, pack or add")),
    };
    planned.map_err(|e| e.to_string())
}

/// Puts the AI's questions to the user; the answer is read with [`ProposalStore::answer_of`].
pub fn ask_user(store: &mut ProposalStore, origin: &Origin, req: &AskUser) -> ToolResult<AskReceipt> {
    let ask = req.checked().map_err(ToolError::Invalid)?;
    let uid = store.ask(origin, &ask)?;
    Ok(AskReceipt { question: uid, message: "Shown to the user in Meshgrove; waiting for their answer.".into() })
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
    fn questions_wait_for_their_answer() {
        let mut store = ProposalStore::open_in_memory().unwrap();
        let o = origin();
        let ask = AskUser {
            questions: vec![
                AskQuestion {
                    question: " Which printer? ".into(),
                    options: vec!["A1 mini".into(), "X1C".into(), "A1 mini".into()],
                    multi: false,
                    allow_other: false,
                },
                AskQuestion { question: "Anything else?".into(), options: vec![], multi: false, allow_other: false },
            ],
        };
        let receipt = ask_user(&mut store, &o, &ask).unwrap();
        let pending = store.pending_questions(Some("mcp"), None).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].uid, receipt.question);
        let q = &pending[0].ask.questions;
        assert_eq!((q[0].question.as_str(), q[0].options.len(), q[0].allow_other), ("Which printer?", 2, false));
        assert!(q[1].allow_other, "no options: an answer in the user's words");
        assert_eq!(store.answer_of(&receipt.question).unwrap(), None);
        store.answer(&receipt.question, &serde_json::json!({"answers": [{"selected": ["X1C"]}]})).unwrap();
        assert_eq!(store.answer_of(&receipt.question).unwrap().unwrap()["answers"][0]["selected"][0], "X1C");
        assert!(store.pending_questions(None, None).unwrap().is_empty());
        assert!(ask_user(&mut store, &o, &AskUser { questions: vec![] }).is_err());
        let many =
            AskUser { questions: vec![AskQuestion { question: "q".into(), ..Default::default() }; MAX_QUESTIONS + 1] };
        assert!(ask_user(&mut store, &o, &many).is_err());
    }

    /// Meshgrove's watcher polls `PRAGMA data_version` on a connection of its own: a question
    /// an assistant writes through another connection has to move it, or the card never shows.
    #[test]
    fn a_question_from_another_connection_moves_the_data_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proposals.db");
        let mut writer = ProposalStore::open(&path).unwrap();
        let watcher = ProposalStore::open(&path).unwrap();
        let before = watcher.data_version().unwrap();
        assert_eq!(watcher.data_version().unwrap(), before, "nothing happened");
        let o = Origin { source: "mcp".into(), client: Some("desktop".into()), class: AccessClass::Cloud };
        let ask = AskUser { questions: vec![AskQuestion { question: "Which?".into(), ..Default::default() }] };
        ask_user(&mut writer, &o, &ask).unwrap();
        assert_ne!(watcher.data_version().unwrap(), before, "the question is a change the watcher sees");
        assert_eq!(watcher.pending_questions(None, None).unwrap().len(), 1);
    }

    #[test]
    fn the_review_can_rewrite_what_was_suggested() {
        let target = Target { path: "/lib/a.stl".into(), hash: None, size: 1, mtime_ns: 2 };
        let note = Proposal {
            id: 1,
            batch_id: 1,
            kind: ProposalKind::Note,
            target: Some(target.clone()),
            payload: serde_json::to_value(NotePayload { text: "old".into(), append: false }).unwrap(),
            reason: None,
            confidence: None,
            state: "pending".into(),
        };
        let picked = Picked { whole: true, note: Some("new".into()), append: Some(true), ..Default::default() };
        let ops = to_ops(&note, &picked, false);
        assert!(matches!(&ops[..], [crate::changes::ApplyOp::SetNote { text, append: true, .. }] if text == "new"));
        let tags = Proposal {
            kind: ProposalKind::Tags,
            payload: serde_json::json!({"add": [{"name": "Garten", "status": "existing"}]}),
            ..note.clone()
        };
        let picked = Picked { add: vec!["Garten".into(), "Rosen".into()], ..Default::default() };
        assert_eq!(to_ops(&tags, &picked, false).len(), 2, "a tag typed in the review is added too");
        let list = Proposal {
            kind: ProposalKind::List,
            target: None,
            payload: serde_json::to_value(ListPayload {
                name: "Geschenke".into(),
                kind: "print".into(),
                items: vec![
                    ListItemPayload { target: target.clone(), quantity: 1 },
                    ListItemPayload { target, quantity: 2 },
                ],
                query: None,
                description: None,
            })
            .unwrap(),
            ..note
        };
        let picked = Picked {
            whole: true,
            list_name: Some(" Gifts ".into()),
            list_kind: Some("collection".into()),
            list_items: Some(vec![(1, 5)]),
            ..Default::default()
        };
        let ops = to_ops(&list, &picked, false);
        assert!(
            matches!(&ops[..], [crate::changes::ApplyOp::List { kind: crate::lists::ListKind::Collection, name, items, .. }] if name == "Gifts" && items.len() == 1 && items[0].1 == 5)
        );
    }

    #[test]
    fn accept_all_keeps_what_was_changed_in_the_review() {
        let target = Target { path: "/a.stl".into(), hash: Some("h".into()), size: 1, mtime_ns: 2 };
        let tags = Proposal {
            id: 1,
            batch_id: 1,
            kind: ProposalKind::Tags,
            target: Some(target.clone()),
            payload: serde_json::json!({"add": [{"name": "Garten", "status": "existing"}, {"name": "Rosen", "status": "new"}], "remove": ["Alt", "Deko"]}),
            reason: None,
            confidence: None,
            state: "pending".into(),
        };
        // Rosen unticked, an own tag typed, and Deko typed back in although the suggestion removes it.
        let edited = Picked {
            add: vec!["Garten".into(), "Prüfteil".into(), "Deko".into()],
            remove: vec!["Alt".into()],
            ..Default::default()
        };
        let all = edited.everything(&tags);
        assert_eq!(all.add, ["Garten", "Rosen", "Prüfteil", "Deko"]);
        assert_eq!(all.remove, ["Alt"]);
        let note = Proposal {
            kind: ProposalKind::Note,
            payload: serde_json::to_value(NotePayload { text: "old".into(), append: false }).unwrap(),
            ..tags.clone()
        };
        let edited = Picked { whole: false, note: Some("rewritten".into()), append: Some(true), ..Default::default() };
        let all = edited.everything(&note);
        assert!(all.whole);
        assert!(
            matches!(&to_ops(&note, &all, false)[..], [crate::changes::ApplyOp::SetNote { text, append: true, .. }] if text == "rewritten")
        );
        let list = Proposal {
            kind: ProposalKind::List,
            target: None,
            payload: serde_json::to_value(ListPayload {
                name: "Geschenke".into(),
                kind: "print".into(),
                items: vec![
                    ListItemPayload { target: target.clone(), quantity: 1 },
                    ListItemPayload { target, quantity: 2 },
                ],
                query: None,
                description: None,
            })
            .unwrap(),
            ..tags
        };
        // The first item unticked, the second one's copies set to 5, the list renamed.
        let edited = Picked { list_name: Some("Gifts".into()), list_items: Some(vec![(1, 5)]), ..Default::default() };
        let all = edited.everything(&list);
        assert_eq!(all.list_items, Some(vec![(0, 1), (1, 5)]));
        assert!(
            matches!(&to_ops(&list, &all, false)[..], [crate::changes::ApplyOp::List { name, items, .. }] if name == "Gifts" && items.len() == 2)
        );
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
