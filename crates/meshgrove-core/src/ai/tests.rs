//! The AI tools against a small library with an open, a local-only and a hidden part: nothing of
//! the hidden part may come out, by any tool, and suggestions never touch the library.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::proposals::*;
use super::read::*;
use super::types::*;
use super::{Context, ToolError};
use crate::cache::ThumbMeta;
use crate::config::{AccessClass, AiAccess, Config, LibraryAi};
use crate::db::Library;
use crate::formats::Kind;
use crate::listing::Entry;
use crate::lists::ListKind;
use crate::printer::{Bed, Printer};

const EIGENE: &str = "/lib/eigene";
const PRIVAT: &str = "/lib/eigene/privat";
const KUNDEN: &str = "/lib/kunden";

fn entry(dir: &str, name: &str) -> Entry {
    Entry {
        name: name.into(),
        path: Path::new(dir).join(name),
        kind: Kind::from_file_name(name),
        size: 100,
        modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1000)),
    }
}

fn config() -> Config {
    let mut c = Config {
        language: Some("de".into()),
        library_roots: [EIGENE, PRIVAT, KUNDEN].map(PathBuf::from).to_vec(),
        ..Default::default()
    };
    c.ai.enabled = true;
    c.ai.libraries = vec![
        LibraryAi { root: EIGENE.into(), access: AiAccess::Cloud, images: true },
        LibraryAi { root: KUNDEN.into(), access: AiAccess::LocalOnly, images: false },
        // PRIVAT is not listed: hidden.
    ];
    c.printers = Some(vec![
        Printer::from(Bed { name: "Kobra S1".into(), x: 250.0, y: 250.0, z: 250.0 }),
        Printer::from(Bed { name: "A1 mini".into(), x: 180.0, y: 180.0, z: 180.0 }),
    ]);
    c
}

/// eigene: rose.stl (Garten, 200 mm), deckel.3mf (Werkstatt, 100 mm), kopie.stl (same content as geheim.stl)
/// privat: geheim.stl, plan.stl (tag "Geheimprojekt", on list "Nur privat")
/// kunden: halter.stl (new, untagged, 50 mm)
/// Tags hang on content, so geheim.stl's tags would show on its visible copy: plan.stl has none.
fn library() -> Library {
    let mut lib = Library::open_in_memory().unwrap();
    for root in [EIGENE, PRIVAT, KUNDEN] {
        lib.finish_baseline(Path::new(root)).unwrap();
    }
    let files: [(&str, &str, &str, f32); 6] = [
        (EIGENE, "rose.stl", "h-rose", 200.0),
        (EIGENE, "deckel.3mf", "h-deckel", 100.0),
        (EIGENE, "kopie.stl", "h-geheim", 30.0),
        (PRIVAT, "geheim.stl", "h-geheim", 30.0),
        (PRIVAT, "plan.stl", "h-plan", 30.0),
        (KUNDEN, "halter.stl", "h-halter", 50.0),
    ];
    for dir in [EIGENE, PRIVAT, KUNDEN] {
        let root = if dir == PRIVAT { Path::new(PRIVAT) } else { Path::new(dir) };
        let entries: Vec<Entry> = files.iter().filter(|f| f.0 == dir).map(|f| entry(f.0, f.1)).collect();
        let subdirs: Vec<PathBuf> = if dir == EIGENE { vec![PRIVAT.into()] } else { Vec::new() };
        lib.update_dir(root, Path::new(dir), 1, &entries, &subdirs).unwrap();
    }
    for (dir, name, hash, size) in files {
        let meta = ThumbMeta { size_mm: Some([size, size / 2.0, 10.0]), content_hash: Some(hash.into()), ..Default::default() };
        lib.store_meta(&Path::new(dir).join(name), "k", &meta).unwrap();
    }
    lib.tag(&[Path::new(EIGENE).join("rose.stl")], "Garten").unwrap();
    lib.tag(&[Path::new(EIGENE).join("deckel.3mf")], "Werkstatt").unwrap();
    lib.tag(&[Path::new(PRIVAT).join("plan.stl")], "Geheimprojekt").unwrap();
    lib.set_note(&[Path::new(EIGENE).join("rose.stl")], "Für die Kletterrose am Zaun").unwrap();
    let secret = lib.create_list(ListKind::Collection, "Nur privat").unwrap();
    lib.add_to_list(secret, &[Path::new(PRIVAT).join("plan.stl")]).unwrap();
    let garden = lib.create_list(ListKind::Print, "Gartenmarkt").unwrap();
    lib.add_to_list(garden, &[Path::new(EIGENE).join("rose.stl")]).unwrap();
    // Only halter.stl came after the baseline: in the inbox.
    lib.conn.execute("UPDATE inbox SET done_ns = 1, done_by = 'baseline' WHERE content_hash != 'h-halter'", []).unwrap();
    lib.set_printers(&config().printers_in_order()).unwrap();
    lib
}

fn ids(page: &Page<ModelSummary>) -> Vec<String> {
    let mut ids: Vec<String> = page.items.iter().map(|m| m.id.clone()).collect();
    ids.sort();
    ids
}

#[test]
fn hidden_libraries_never_show() {
    let (lib, config) = (library(), config());
    let cx = Context::new(&lib, &config, AccessClass::Cloud, None).unwrap();
    let all = search_models(&cx, &ModelFilter::default()).unwrap();
    assert_eq!(ids(&all), ["eigene/deckel.3mf", "eigene/kopie.stl", "eigene/rose.stl"]);
    for q in ["plan", "tag:geheimprojekt", "liste:\"nur privat\"", "privat"] {
        let found = search_models(&cx, &ModelFilter { q: Some(q.into()), ..Default::default() }).unwrap();
        assert!(found.items.is_empty(), "{q}: {:?}", ids(&found));
    }
    let tags = list_tags(&cx).unwrap();
    let names: Vec<&str> = tags.existing.iter().map(|t| t.name.as_str()).collect();
    assert!(!names.contains(&"Geheimprojekt"), "a tag only hidden files carry stays hidden: {names:?}");
    let lists: Vec<String> = list_lists(&cx).unwrap().into_iter().map(|l| l.name).collect();
    assert!(lists.contains(&"Gartenmarkt".to_owned()) && !lists.contains(&"Nur privat".to_owned()), "{lists:?}");
    assert!(matches!(get_list(&cx, "Nur privat"), Err(ToolError::UnknownId(_))));
    let tagged = search_models(&cx, &ModelFilter { tags_all: vec!["Geheimprojekt".into()], ..Default::default() }).unwrap();
    assert!(tagged.items.is_empty());
    assert!(find_duplicates(&cx, None, 50).unwrap().is_empty(), "the hidden copy is no duplicate to see");
    let (detail, _) = get_model(&cx, "eigene/kopie.stl", false).unwrap();
    assert!(detail.copies.is_empty(), "{:?}", detail.copies);
    let overview = library_overview(&cx).unwrap();
    assert_eq!(overview.libraries.iter().map(|l| (l.alias.as_str(), l.files)).collect::<Vec<_>>(), [("eigene", 3)]);
    assert_eq!(overview.inbox_waiting, 0, "the new file is in a library cloud AI may not see");
    // Hidden or unknown: the same answer.
    let hidden = get_model(&cx, "privat/geheim.stl", false).unwrap_err().to_string();
    let unknown = get_model(&cx, "gibtsnicht/x.stl", false).unwrap_err().to_string();
    assert_eq!(hidden.replace("privat/geheim.stl", "X"), unknown.replace("gibtsnicht/x.stl", "X"));
    assert!(get_model(&cx, "eigene/privat/geheim.stl", false).is_err(), "nor through the library around it");
}

#[test]
fn local_ai_sees_the_local_library_but_no_pictures_of_it() {
    let (lib, config) = (library(), config());
    let cx = Context::new(&lib, &config, AccessClass::Local, None).unwrap();
    let new = search_models(&cx, &ModelFilter { new_only: true, ..Default::default() }).unwrap();
    assert_eq!(ids(&new), ["kunden/halter.stl"]);
    assert!(new.items[0].new && new.items[0].tags.is_empty());
    let inbox = list_inbox(&cx, None, 99).unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].folder, "kunden");
    let (pictures, denied) = get_thumbnails(&cx, &["kunden/halter.stl".into()], 256).unwrap();
    assert!(pictures.is_empty() && denied == ["kunden/halter.stl"]);
    let (detail, _) = get_model(&cx, "kunden/halter.stl", true).unwrap();
    assert!(!detail.thumbnail_allowed);
    assert_eq!(detail.path, "/lib/kunden/halter.stl", "the full path, for slicers and printer servers");
    let off = Config { ai: crate::config::AiConfig { enabled: false, ..config.ai.clone() }, ..config.clone() };
    assert!(matches!(Context::new(&lib, &off, AccessClass::Local, None), Err(ToolError::Disabled)));
}

#[test]
fn filters_and_facts() {
    let (lib, config) = (library(), config());
    let cx = Context::new(&lib, &config, AccessClass::Local, None).unwrap();
    let search = |f: ModelFilter| ids(&search_models(&cx, &f).unwrap());
    assert_eq!(search(ModelFilter { fits_printer: Some("A1 mini".into()), ..Default::default() }), [
        "eigene/deckel.3mf",
        "eigene/kopie.stl",
        "kunden/halter.stl"
    ]);
    assert_eq!(search(ModelFilter { max_size_mm: Some(60.0), ..Default::default() }), ["eigene/kopie.stl", "kunden/halter.stl"]);
    assert_eq!(search(ModelFilter { tags_any: vec!["garten".into(), "Werkstatt".into()], ..Default::default() }), [
        "eigene/deckel.3mf",
        "eigene/rose.stl"
    ]);
    assert_eq!(search(ModelFilter { tags_none: vec!["Garten".into()], library: Some("eigene".into()), ..Default::default() }), [
        "eigene/deckel.3mf",
        "eigene/kopie.stl"
    ]);
    assert!(matches!(
        search_models(&cx, &ModelFilter { kinds: vec!["banana".into()], ..Default::default() }),
        Err(ToolError::Invalid(_))
    ));
    let page = search_models(&cx, &ModelFilter { limit: Some(2), ..Default::default() }).unwrap();
    assert_eq!(page.items.len(), 2);
    let rest = search_models(&cx, &ModelFilter { cursor: page.next_cursor.clone(), ..Default::default() }).unwrap();
    assert_eq!(rest.items.len(), 2, "4 models in all");
    let (rose, _) = get_model(&cx, "eigene/rose.stl", false).unwrap();
    assert_eq!(rose.summary.fits, ["Kobra S1"]);
    assert_eq!(rose.summary.too_big_for, ["A1 mini"]);
    assert_eq!(rose.note.as_deref(), Some("Für die Kletterrose am Zaun"));
    assert_eq!(rose.lists, ["Gartenmarkt"]);
    let folder = folder_summary(&cx, "eigene").unwrap();
    assert_eq!(folder.files, 3, "privat below it is hidden");
    assert!(folder.subfolders.is_empty(), "{:?}", folder.subfolders);
    let printers = list_printers(&cx);
    assert!(printers[0].main && printers[0].name == "Kobra S1");
    let checks = check_printability(&cx, &["eigene/rose.stl".into(), "privat/geheim.stl".into()]).unwrap();
    assert_eq!(checks[1].problems, ["unknown_id"]);
}

fn file_tags(lib: &Library) -> i64 {
    lib.conn.query_row("SELECT COUNT(*) FROM file_tags", [], |r| r.get(0)).unwrap()
}

#[test]
fn suggestions_wait_and_leave_the_library_alone() {
    let (lib, config) = (library(), config());
    let cx = Context::new(&lib, &config, AccessClass::Local, None).unwrap();
    let mut store = ProposalStore::open_in_memory().unwrap();
    let origin = Origin { source: "mcp".into(), client: Some("test".into()), class: AccessClass::Local };
    let before = file_tags(&lib);
    let receipt = propose_tags(
        &cx,
        &mut store,
        &origin,
        &ProposeTags {
            title: "Kunde einordnen".into(),
            items: vec![
                TagItem { id: "kunden/halter.stl".into(), add: vec!["werkstatt".into(), "Workshop".into(), "Halterung".into()], ..Default::default() },
                TagItem { id: "privat/geheim.stl".into(), add: vec!["x".into()], ..Default::default() },
                TagItem { id: "eigene/rose.stl".into(), add: vec!["zu\nlang".into()], ..Default::default() },
            ],
            mark_inbox_done: true,
        },
    )
    .unwrap();
    assert_eq!(receipt.items, 1);
    assert_eq!(receipt.rejected.len(), 2);
    assert_eq!(
        receipt.tags.iter().map(|t| (t.name.as_str(), t.status)).collect::<Vec<_>>(),
        [("Werkstatt", super::vocab::TagStatus::Existing), ("Halterung", super::vocab::TagStatus::New)],
        "spelling of the library, translations folded, the new one marked"
    );
    assert_eq!(file_tags(&lib), before, "nothing changed in the library");
    let batches = store.batches(Some("mcp"), Some("test"), &["pending"], 9).unwrap();
    assert_eq!(batches.len(), 1);
    assert!(batches[0].inbox_done);
    let p = &store.proposals(batches[0].id).unwrap()[0];
    let target = p.target.as_ref().unwrap();
    assert_eq!((target.path.as_path(), target.hash.as_deref()), (Path::new("/lib/kunden/halter.stl"), Some("h-halter")));

    let list = propose_list(
        &cx,
        &mut store,
        &origin,
        &ProposeList {
            name: "Wochenende".into(),
            items: vec![ListItemIn { id: "eigene/rose.stl".into(), quantity: Some(3) }],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(list.items, 1);
    let merge = propose_tag_merge(&cx, &mut store, &origin, &ProposeTagMerge { from: vec!["Werkstatt".into()], to: "Werkbank".into() })
        .unwrap();
    assert_eq!(merge.items, 1);
    assert_eq!(store.pending_count().unwrap(), 3);
    assert_eq!(file_tags(&lib), before);
    assert!(lib.lists().unwrap().iter().all(|l| l.name != "Wochenende"));
}

#[test]
fn print_time_comes_from_the_gcode_sliced_from_a_model() {
    let mut lib = Library::open_in_memory().unwrap();
    lib.finish_baseline(Path::new(EIGENE)).unwrap();
    let names = ["rose.stl", "rose_PLA_1h.gcode", "rosen.stl", "lang.stl", "lang.gcode"];
    let entries: Vec<Entry> = names.iter().map(|n| entry(EIGENE, n)).collect();
    lib.update_dir(Path::new(EIGENE), Path::new(EIGENE), 1, &entries, &[]).unwrap();
    for (name, time) in [("rose_PLA_1h.gcode", "1h 0m 0s"), ("lang.gcode", "3h 10m")] {
        let gcode = crate::loaders::gcode::GcodeFacts { print_time: Some(time.into()), ..Default::default() };
        let meta = ThumbMeta { gcode: Some(Box::new(gcode)), content_hash: Some(format!("h-{name}")), ..Default::default() };
        lib.store_meta(&Path::new(EIGENE).join(name), "k", &meta).unwrap();
    }
    let config = config();
    let cx = Context::new(&lib, &config, AccessClass::Cloud, None).unwrap();
    let quick = search_models(&cx, &ModelFilter { max_print_minutes: Some(120), ..Default::default() }).unwrap();
    assert_eq!(ids(&quick), ["eigene/rose.stl", "eigene/rose_PLA_1h.gcode"], "not rosen.stl, not the long one");
}
