//! The one-click actions, run on the chat thread: tags for files (an inbox drop, the selection)
//! and a list from a description. Each asks the model for JSON only (see
//! `meshgrove_core::ai::assist`), checks the answer and leaves suggestions in the review; the
//! chat gets a card for them and what was suggested, to ask about or change.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use meshgrove_core::ai::assist::{self, Prompt};
use meshgrove_core::ai::proposals::{self, MAX_ITEMS, Origin, ProposalStore, ProposeList, ProposeTags};
use meshgrove_core::ai::types::FolderSummary;
use meshgrove_core::ai::vocab::Vocabulary;
use meshgrove_core::ai::{Context, read};
use meshgrove_core::cache::ThumbCache;
use meshgrove_core::config::{AccessClass, Config, EndpointConfig};
use meshgrove_core::db::Library;
use serde_json::{Value, json};

use super::chat::ChatEvent;
use super::client::Endpoint;

/// Files that came together, with where from (its readme tells the model about them).
#[derive(Debug, Clone)]
pub struct TagGroup {
    /// The suggestions' title in the review.
    pub title: String,
    pub folder: Option<PathBuf>,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum Action {
    /// Tags for files, one batch of suggestions per group; with `inbox_done` accepting them also
    /// takes the files out of the inbox.
    Tags { groups: Vec<TagGroup>, inbox_done: bool },
    /// A print list (or a collection) from what the user described.
    List { description: String, print: bool },
}

pub struct ActionJob {
    pub action: Action,
    pub config: Config,
    pub key: Option<String>,
}

/// Most suggestions written out in the chat.
const SHOWN: usize = 30;
/// Thumbnail size sent along.
const PICTURE: u32 = 256;

/// What a run ends with: the text for the chat, or nothing when it was stopped.
type Outcome = Result<Option<String>, String>;

pub(super) fn run(db: &Path, job: ActionJob, cancel: &AtomicBool, send: &dyn Fn(ChatEvent)) {
    match ProposalStore::default_location() {
        Some(store) => run_with(db, &store, job, cancel, send),
        None => send(ChatEvent::Failed("no data folder".into())),
    }
}

/// `run` with the suggestions going to the store at `store`.
fn run_with(db: &Path, store: &Path, job: ActionJob, cancel: &AtomicBool, send: &dyn Fn(ChatEvent)) {
    let result = match &job.action {
        Action::Tags { groups, inbox_done } => tags(db, store, &job, groups, *inbox_done, cancel, send),
        Action::List { description, print } => list(db, store, &job, description, *print, cancel, send),
    };
    match result {
        Ok(text) => {
            if let Some(text) = text {
                send(ChatEvent::Text(text));
            }
            send(ChatEvent::Done(String::new()));
        }
        Err(e) => send(ChatEvent::Failed(e)),
    }
}

fn class_of(endpoint: &EndpointConfig) -> AccessClass {
    if endpoint.local { AccessClass::Local } else { AccessClass::Cloud }
}

fn open_library(db: &Path, config: &Config) -> Result<Library, String> {
    let mut lib = Library::open_reader(db).ok_or_else(|| t!("chat.no_index").into_owned())?;
    lib.set_printers(&config.printers_in_order()).map_err(|e| e.to_string())?;
    Ok(lib)
}

fn open_store(path: &Path) -> Result<ProposalStore, String> {
    ProposalStore::open(path).map_err(|e| e.to_string())
}

fn endpoint_of(job: &ActionJob) -> Result<(&EndpointConfig, Endpoint), String> {
    let cfg = job.config.ai.endpoint.as_ref().ok_or_else(|| t!("chat.no_endpoint").into_owned())?;
    Ok((cfg, Endpoint::from_config(cfg, job.key.clone())))
}

/// The user's text with the pictures after it, each named by its file's id.
fn content(text: &str, pictures: &[(String, Vec<u8>)]) -> Value {
    if pictures.is_empty() {
        return json!(text);
    }
    let mut parts = vec![json!({"type": "text", "text": text})];
    for (id, png) in pictures {
        parts.push(json!({"type": "text", "text": format!("Picture of {id}:")}));
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        parts.push(json!({"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{data}")}}));
    }
    Value::Array(parts)
}

/// Asks for JSON and checks it with `parse`; once more, told what was wrong, when it does not pass.
fn ask<T>(
    endpoint: &Endpoint,
    prompt: &Prompt,
    pictures: &[(String, Vec<u8>)],
    schema: &Value,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<T, String> {
    let mut messages = vec![
        json!({"role": "system", "content": prompt.system}),
        json!({"role": "user", "content": content(&prompt.user, pictures)}),
    ];
    let answer = endpoint.complete_json(&messages, schema).map_err(|e| e.to_string())?;
    match parse(&answer) {
        Ok(v) => Ok(v),
        Err(problem) => {
            log::info!("the model's answer did not pass ({problem}), asking again");
            messages.push(json!({"role": "assistant", "content": answer}));
            messages.push(json!({
                "role": "user",
                "content": format!("That answer cannot be used: {problem}. Answer again, with the JSON only.")
            }));
            let answer = endpoint.complete_json(&messages, schema).map_err(|e| e.to_string())?;
            parse(&answer)
        }
    }
}

/// What the model is told about the folder files came in.
fn folder_context(s: &FolderSummary) -> String {
    let mut out = format!("Folder: {}", s.id);
    if !s.subfolders.is_empty() {
        out.push_str(&format!("\nSubfolders: {}", s.subfolders.join(", ")));
    }
    if !s.common_tags.is_empty() {
        let tags: Vec<&str> = s.common_tags.iter().map(|t| t.name.as_str()).collect();
        out.push_str(&format!("\nTags most of its files carry: {}", tags.join(", ")));
    }
    if let Some(readme) = &s.readme_untrusted {
        out.push_str(&format!("\nIts readme: {readme}"));
    }
    out
}

/// "- [[id]]: Garten, Rankhilfe" lines, at most [`SHOWN`], then how many more.
fn listing(lines: Vec<String>) -> String {
    let more = lines.len().saturating_sub(SHOWN);
    let mut out: Vec<String> = lines.into_iter().take(SHOWN).collect();
    if more > 0 {
        out.push(t!("actions.more", count = more).into_owned());
    }
    out.join("\n")
}

fn tags(
    db: &Path,
    store: &Path,
    job: &ActionJob,
    groups: &[TagGroup],
    inbox_done: bool,
    cancel: &AtomicBool,
    send: &dyn Fn(ChatEvent),
) -> Outcome {
    let (cfg, endpoint) = endpoint_of(job)?;
    let lib = open_library(db, &job.config)?;
    let thumbs = ThumbCache::default_location().map(ThumbCache::new);
    let class = class_of(cfg);
    let cx = Context::new(&lib, &job.config, class, thumbs.as_ref()).map_err(|e| e.to_string())?;
    let vocab = read::list_tags(&cx).map_err(|e| e.to_string())?;
    let spelling = Vocabulary::new(&job.config, vocab.existing.iter().map(|t| t.name.clone()).collect());
    let mut store = open_store(store)?;
    let origin = Origin { source: "endpoint".into(), client: Some(cfg.model.clone()), class };
    let batch = if cfg.vision { assist::BATCH_WITH_PICTURES } else { assist::BATCH };
    let (mut total, mut hidden) = (0, 0);
    let mut lines = Vec::new();
    for group in groups {
        let files = read::summaries(&cx, &group.paths).map_err(|e| e.to_string())?;
        hidden += group.paths.len() - files.len();
        total += files.len();
        let context = group
            .folder
            .as_ref()
            .and_then(|f| cx.scope.id_of(f))
            .and_then(|id| read::folder_summary(&cx, &id).ok())
            .map(|s| folder_context(&s));
        let mut items = Vec::new();
        for chunk in files.chunks(batch) {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let ids: Vec<String> = chunk.iter().map(|f| f.id.clone()).collect();
            // Pictures only where the library allows them (the others are left out).
            let pictures =
                if cfg.vision { read::get_thumbnails(&cx, &ids, PICTURE).map(|(p, _)| p).unwrap_or_default() } else { Vec::new() };
            let prompt = assist::tag_prompt(chunk, &vocab, context.as_deref());
            match ask(&endpoint, &prompt, &pictures, &assist::tag_schema(), |a| assist::parse_tags(a, &ids)) {
                Ok(mut found) => {
                    // Only what the files do not carry yet.
                    for item in &mut found {
                        let has = chunk.iter().find(|f| f.id == item.id).map(|f| f.tags.as_slice()).unwrap_or_default();
                        item.add.retain(|t| !has.iter().any(|h| h.eq_ignore_ascii_case(&spelling.normalize(t).0)));
                    }
                    found.retain(|i| !i.add.is_empty());
                    let summary = t!("actions.step_tags", tagged = found.len(), count = chunk.len()).into_owned();
                    send(ChatEvent::Tool { name: "tags".into(), summary });
                    items.extend(found);
                }
                Err(e) => {
                    send(ChatEvent::Tool { name: "tags".into(), summary: t!("chat.tool_failed", error = e).into_owned() })
                }
            }
        }
        for part in items.chunks(MAX_ITEMS) {
            let request = ProposeTags { title: group.title.clone(), items: part.to_vec(), mark_inbox_done: inbox_done };
            let receipt = proposals::propose_tags(&cx, &mut store, &origin, &request).map_err(|e| e.to_string())?;
            send(ChatEvent::Proposed { batch: receipt.batch, title: group.title.clone(), items: receipt.items });
        }
        for item in &items {
            let tags: Vec<String> = item.add.iter().map(|t| spelling.normalize(t).0).collect();
            lines.push(format!("- [[{}]]: {}", item.id, tags.join(", ")));
        }
    }
    if total == 0 {
        return Err(t!("actions.nothing_visible").into_owned());
    }
    if lines.is_empty() {
        return Err(t!("actions.no_tags").into_owned());
    }
    let mut text = match (lines.len(), total) {
        (1, 1) => t!("actions.tags_done_one").into_owned(),
        (tagged, count) => t!("actions.tags_done", tagged = tagged, count = count).into_owned(),
    };
    if hidden > 0 {
        text.push(' ');
        text.push_str(&t!("actions.hidden", count = hidden));
    }
    Ok(Some(format!("{text}\n\n{}", listing(lines))))
}

fn list(
    db: &Path,
    store: &Path,
    job: &ActionJob,
    description: &str,
    print: bool,
    cancel: &AtomicBool,
    send: &dyn Fn(ChatEvent),
) -> Outcome {
    let (cfg, endpoint) = endpoint_of(job)?;
    let lib = open_library(db, &job.config)?;
    let class = class_of(cfg);
    let cx = Context::new(&lib, &job.config, class, None).map_err(|e| e.to_string())?;
    let vocab = read::list_tags(&cx).map_err(|e| e.to_string())?;
    let printers = read::list_printers(&cx);
    // 1. The description as a search.
    let prompt = assist::filter_prompt(description, &vocab, &printers);
    let plan = ask(&endpoint, &prompt, &[], &assist::filter_schema(), assist::parse_filter)?;
    let wanted = assist::describe_filter(&plan.filter);
    send(ChatEvent::Tool { name: "filter".into(), summary: wanted.clone() });
    let mut found = Vec::new();
    for filter in assist::loosened(&plan.filter) {
        match read::search_models(&cx, &filter) {
            Ok(page) if !page.items.is_empty() => {
                found = page.items;
                break;
            }
            Ok(_) => {}
            Err(e) => send(ChatEvent::Tool {
                name: "search_models".into(),
                summary: t!("chat.tool_failed", error = e.to_string()).into_owned(),
            }),
        }
    }
    send(ChatEvent::Tool { name: "search_models".into(), summary: t!("chat.tool_results", count = found.len()).into_owned() });
    if found.is_empty() {
        return Ok(Some(t!("actions.list_nothing", search = wanted).into_owned()));
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    // 2. The files that fit, picked from what was found.
    let ids: Vec<String> = found.iter().map(|f| f.id.clone()).collect();
    let mut items = if found.len() == 1 {
        vec![proposals::ListItemIn { id: ids[0].clone(), quantity: None }]
    } else {
        let prompt = assist::pick_prompt(description, &found, print);
        match ask(&endpoint, &prompt, &[], &assist::pick_schema(), |a| assist::parse_pick(a, &ids)) {
            Ok(items) => {
                let summary = t!("actions.step_pick", picked = items.len(), count = found.len()).into_owned();
                send(ChatEvent::Tool { name: "pick".into(), summary });
                items
            }
            Err(e) => {
                send(ChatEvent::Notice(t!("actions.pick_failed", error = e).into_owned()));
                ids.iter().map(|id| proposals::ListItemIn { id: id.clone(), quantity: None }).collect()
            }
        }
    };
    if items.is_empty() {
        return Ok(Some(t!("actions.list_none_fits", count = found.len()).into_owned()));
    }
    if print {
        for item in &mut items {
            item.quantity = item.quantity.or(plan.quantity);
        }
    }
    let name = plan.name.clone().unwrap_or_else(|| description.trim().chars().take(60).collect());
    let request = ProposeList {
        name: name.clone(),
        kind: Some(if print { "print" } else { "collection" }.into()),
        items: items.clone(),
        query: None,
        description: Some(description.trim().chars().take(300).collect()),
    };
    let mut store = open_store(store)?;
    let origin = Origin { source: "endpoint".into(), client: Some(cfg.model.clone()), class };
    let receipt = proposals::propose_list(&cx, &mut store, &origin, &request).map_err(|e| e.to_string())?;
    send(ChatEvent::Proposed { batch: receipt.batch, title: name.clone(), items: receipt.items });
    let lines = items
        .iter()
        .map(|i| match i.quantity.filter(|q| *q > 1 && print) {
            Some(q) => format!("- [[{}]] ×{q}", i.id),
            None => format!("- [[{}]]", i.id),
        })
        .collect();
    let text = t!("actions.list_done", name = name, count = items.len(), found = found.len());
    Ok(Some(format!("{text}\n\n{}", listing(lines))))
}

/// Tags for the tag editor, suggested for these files (at most one request's worth): those
/// suggested most often first, spelled as the library spells them. Nothing is stored.
pub fn tag_candidates(db: &Path, config: &Config, key: Option<String>, paths: &[PathBuf]) -> Result<Vec<String>, String> {
    let cfg = config.ai.endpoint.as_ref().ok_or_else(|| t!("chat.no_endpoint").into_owned())?;
    let endpoint = Endpoint::from_config(cfg, key);
    let lib = open_library(db, config)?;
    let thumbs = ThumbCache::default_location().map(ThumbCache::new);
    let cx = Context::new(&lib, config, class_of(cfg), thumbs.as_ref()).map_err(|e| e.to_string())?;
    let batch = if cfg.vision { assist::BATCH_WITH_PICTURES } else { assist::BATCH };
    let files = read::summaries(&cx, &paths[..paths.len().min(batch)]).map_err(|e| e.to_string())?;
    if files.is_empty() {
        return Err(t!("actions.nothing_visible").into_owned());
    }
    let vocab = read::list_tags(&cx).map_err(|e| e.to_string())?;
    let spelling = Vocabulary::new(config, vocab.existing.iter().map(|t| t.name.clone()).collect());
    let ids: Vec<String> = files.iter().map(|f| f.id.clone()).collect();
    let pictures = if cfg.vision { read::get_thumbnails(&cx, &ids, PICTURE).map(|(p, _)| p).unwrap_or_default() } else { Vec::new() };
    let folder = paths.first().and_then(|p| p.parent()).filter(|f| paths.iter().all(|p| p.parent() == Some(*f)));
    let context = folder.and_then(|f| cx.scope.id_of(f)).and_then(|id| read::folder_summary(&cx, &id).ok()).map(|s| folder_context(&s));
    let prompt = assist::tag_prompt(&files, &vocab, context.as_deref());
    let items = ask(&endpoint, &prompt, &pictures, &assist::tag_schema(), |a| assist::parse_tags(a, &ids))?;
    let mut counted: Vec<(String, usize)> = Vec::new();
    for tag in items.iter().flat_map(|i| &i.add) {
        let tag = spelling.normalize(tag).0;
        match counted.iter_mut().find(|(t, _)| t.eq_ignore_ascii_case(&tag)) {
            Some((_, n)) => *n += 1,
            None => counted.push((tag, 1)),
        }
    }
    counted.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    Ok(counted.into_iter().map(|(t, _)| t).take(8).collect())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime};

    use meshgrove_core::Kind;
    use meshgrove_core::cache::ThumbMeta;
    use meshgrove_core::config::{AiAccess, EndpointConfig, LibraryAi};
    use meshgrove_core::listing::Entry;

    use super::*;
    use crate::assist::client::tests::serve;

    struct Fixture {
        _dir: tempfile::TempDir,
        db: PathBuf,
        store: PathBuf,
        open: PathBuf,
        config: Config,
    }

    /// offen (cloud): rose.stl (tagged "Garten"), deckel.stl; privat (no AI): geheim.stl.
    fn fixture(url: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let (open, hidden) = (dir.path().join("offen"), dir.path().join("privat"));
        let db = dir.path().join("library.db");
        let mut config = Config { library_roots: vec![open.clone(), hidden.clone()], language: Some("de".into()), ..Default::default() };
        config.ai.enabled = true;
        config.ai.libraries = vec![LibraryAi { root: open.clone(), access: AiAccess::Cloud, images: false }];
        config.ai.endpoint = Some(EndpointConfig { base_url: url.into(), model: "mock".into(), local: false, ..Default::default() });
        let mut lib = Library::open(&db).unwrap();
        for (root, names) in [(&open, &["rose.stl", "deckel.stl"][..]), (&hidden, &["geheim.stl"][..])] {
            lib.finish_baseline(root).unwrap();
            let entries: Vec<Entry> = names
                .iter()
                .map(|n| Entry {
                    name: (*n).into(),
                    path: root.join(n),
                    kind: Kind::from_file_name(n),
                    size: 100,
                    modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1000)),
                })
                .collect();
            lib.update_dir(root, root, 1, &entries, &[]).unwrap();
            for n in names {
                let meta = ThumbMeta { size_mm: Some([40.0, 20.0, 10.0]), content_hash: Some(format!("h-{n}")), ..Default::default() };
                lib.store_meta(&root.join(n), "k", &meta).unwrap();
            }
        }
        lib.tag(&[open.join("rose.stl")], "Garten").unwrap();
        let store = dir.path().join("proposals.db");
        Fixture { _dir: dir, db, store, open, config }
    }

    fn answer(content: Value) -> (u16, &'static str, String) {
        (200, "application/json", json!({"choices": [{"message": {"content": content.to_string()}}]}).to_string())
    }

    fn run_job(f: &Fixture, action: Action) -> Vec<ChatEvent> {
        let events = Mutex::new(Vec::new());
        let job = ActionJob { action, config: f.config.clone(), key: None };
        run_with(&f.db, &f.store, job, &AtomicBool::new(false), &|e| events.lock().unwrap().push(e));
        events.into_inner().unwrap()
    }

    fn body(request: &str) -> Value {
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    }

    #[test]
    fn tags_for_an_inbox_drop_become_one_batch_of_suggestions() {
        // The first answer uses an id that was never sent and no usable tags: asked again.
        let (url, seen) = serve(vec![
            answer(json!({"items": [{"id": "privat/geheim.stl", "tags": ["Fremd"]}]})),
            answer(json!({"items": [
                {"id": "offen/deckel.stl", "tags": ["werkstatt", "Ersatzteil"], "reason": "Deckel"},
                {"id": "privat/geheim.stl", "tags": ["Fremd"]}
            ]})),
        ]);
        let f = fixture(&url);
        let group = TagGroup {
            title: "Eingang: offen".into(),
            folder: Some(f.open.clone()),
            paths: vec![f.open.join("deckel.stl"), f.config.library_roots[1].join("geheim.stl")],
        };
        let events = run_job(&f, Action::Tags { groups: vec![group], inbox_done: true });
        let proposed: Vec<&ChatEvent> = events.iter().filter(|e| matches!(e, ChatEvent::Proposed { .. })).collect();
        assert_eq!(proposed.len(), 1, "{events:?}");
        let Some(ChatEvent::Text(text)) = events.iter().find(|e| matches!(e, ChatEvent::Text(_))) else { panic!("{events:?}") };
        // "werkstatt" as Meshgrove's suggested tag spells it.
        assert!(text.contains("[[offen/deckel.stl]]: Werkstatt, Ersatzteil") && !text.contains("geheim"), "{text}");
        assert!(matches!(events.last(), Some(ChatEvent::Done(_))));

        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let first = body(&requests[0]);
        let prompt = first.to_string();
        assert!(prompt.contains("offen/deckel.stl") && prompt.contains("Garten (1)"), "the file and the tags in use");
        assert!(!prompt.contains("geheim"), "nothing of the hidden library goes out");
        assert!(body(&requests[1])["messages"].as_array().unwrap().len() == 4, "asked again with what was wrong");

        let store = ProposalStore::open(&f.store).unwrap();
        let batches = store.batches(None, None, &[], 10).unwrap();
        assert_eq!(batches.len(), 1);
        assert!(batches[0].inbox_done && batches[0].title == "Eingang: offen");
        let proposals = store.proposals(batches[0].id).unwrap();
        assert_eq!(proposals.len(), 1, "only the visible file");
    }

    #[test]
    fn a_list_from_a_description_searches_then_picks() {
        let (url, seen) = serve(vec![
            answer(json!({"name": "Gartenmarkt", "tags_any": ["Garten"], "q": "gibtsnicht", "quantity": 3})),
            answer(json!({"items": [{"id": "offen/rose.stl"}, {"id": "privat/geheim.stl"}]})),
        ]);
        let f = fixture(&url);
        let events = run_job(&f, Action::List { description: "Alles für den Gartenmarkt, je 3 Stück".into(), print: true });
        let Some(ChatEvent::Text(text)) = events.iter().find(|e| matches!(e, ChatEvent::Text(_))) else { panic!("{events:?}") };
        assert!(text.contains("Gartenmarkt") && text.contains("[[offen/rose.stl]] ×3"), "{text}");
        let requests = seen.lock().unwrap();
        // The words found nothing, so the search went on without them; only one file was found:
        // nothing to pick, one request.
        assert_eq!(requests.len(), 1, "{events:?}");
        let store = ProposalStore::open(&f.store).unwrap();
        let batches = store.batches(None, None, &[], 10).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(store.proposals(batches[0].id).unwrap().len(), 1);
    }
}
