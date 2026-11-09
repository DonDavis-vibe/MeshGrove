//! The library as plugins read it ([`LibraryReader`]), on a library in a temporary folder: no
//! plugin is needed for these.

mod support;

use std::path::PathBuf;

use meshgrove_core::config::{AccessClass, AiAccess, Config, LibraryAi, LibraryRead};
use meshgrove_core::db::Library;
use meshgrove_plugins::library::{part_token, split_part};
use meshgrove_plugins::{Caller, LibraryAccess, LibraryReader};
use serde_json::json;

/// A library "lib" with a box and a 3MF of two objects, indexed, and the settings naming it.
struct Scene {
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    reader: LibraryReader,
}

fn scene() -> Scene {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    std::fs::create_dir_all(root.join("Dosen")).unwrap();
    std::fs::write(root.join("box.stl"), support::stl([10.0, 20.0, 30.0])).unwrap();
    std::fs::write(root.join("Dosen").join("dose.3mf"), support::dose_3mf()).unwrap();
    let db = dir.path().join("library.db");
    let mut lib = Library::open(&db).unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    meshgrove_core::scan::scan(&mut lib, &root, &root, meshgrove_core::scan::Force::Nothing, &cancel).unwrap();
    drop(lib);
    let config = dir.path().join("config.toml");
    Config { library_roots: vec![root.clone()], ..Config::default() }.save_to(&config).unwrap();
    let reader = LibraryReader::new(db, Some(config.clone()));
    Scene { _dir: dir, root, config, reader }
}

const ALL: (LibraryRead, Caller) = (LibraryRead::All, Caller::User);

#[test]
fn a_3mf_says_its_parts_and_gives_each_by_its_token() {
    let s = scene();
    let facts = s.reader.describe("lib/Dosen/dose.3mf", ALL.0, ALL.1).unwrap();
    assert!(facts.get("path").is_none(), "no paths: {facts}");
    let parts = facts["parts"].as_array().expect("a 3MF lists its parts");
    assert_eq!(parts.len(), 2, "the lid is placed twice and is one part");
    assert_eq!((parts[0]["name"].as_str(), parts[1]["name"].as_str()), (Some("Dose"), Some("Deckel")));
    assert_eq!((parts[0]["triangles"].as_u64(), parts[1]["instances"].as_u64()), (Some(12), Some(2)));
    assert_eq!((&parts[1]["min"], &parts[1]["max"]), (&json!([0.0, 0.0, 0.0]), &json!([40.0, 30.0, 2.0])));
    let shift = json!([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 50.0, 0.0, 0.0]);
    assert_eq!(parts[1]["transform"], shift, "where it is placed first");
    let token = parts[1]["token"].as_str().unwrap();
    assert_eq!(token, part_token("lib/Dosen/dose.3mf", 1));
    assert_eq!(split_part(token), Ok(("lib/Dosen/dose.3mf", Some(1))));

    // The part by itself, where its own coordinates put it; the file as a whole, every placement.
    let lid = s.reader.triangles("lib/Dosen/dose.3mf", Some(1), 100_000, ALL.0, ALL.1).unwrap();
    assert_eq!((lid.triangles(), lid.vertices()), (12, 8));
    assert_eq!(lid.bounds(), Some(([0.0; 3], [40.0, 30.0, 2.0])));
    let whole = s.reader.triangles("lib/Dosen/dose.3mf", None, 100_000, ALL.0, ALL.1).unwrap();
    assert_eq!((whole.triangles(), whole.bounds().unwrap().1), (36, [140.0, 30.0, 20.0]));
    let none = s.reader.triangles("lib/Dosen/dose.3mf", Some(2), 100_000, ALL.0, ALL.1).unwrap_err();
    assert!(none.contains("has 2 parts"), "{none}");
    // Other files have no parts.
    assert!(s.reader.describe("lib/box.stl", ALL.0, ALL.1).unwrap().get("parts").is_none());
    assert!(s.reader.triangles("lib/box.stl", Some(0), 100, ALL.0, ALL.1).is_err());
    assert_eq!(s.reader.triangles("lib/box.stl", None, 100, ALL.0, ALL.1).unwrap().triangles(), 12);
}

/// "What local AI sees" is what the user opened to local AI: a panel reads it also while AI is
/// switched off, a tool call (an AI) does not.
#[test]
fn the_grant_reaches_as_far_as_the_user_set_it() {
    let s = scene();
    let read = |read, caller| s.reader.triangles("lib/box.stl", None, 100, read, caller).map(|m| m.triangles());
    let local_ai = Caller::Ai(AccessClass::Local);
    // A new library is closed to AI: only "everything" reaches it.
    assert_eq!(read(LibraryRead::All, Caller::User), Ok(12));
    assert!(read(LibraryRead::AiScope, Caller::User).unwrap_err().contains("unknown id"));
    assert!(read(LibraryRead::None, Caller::User).is_err());
    // Opened to local AI, with AI still switched off.
    let mut config = Config::load_from(&s.config).unwrap();
    config.ai.libraries = vec![LibraryAi { root: s.root.clone(), access: AiAccess::LocalOnly, images: false }];
    config.save_to(&s.config).unwrap();
    assert_eq!(read(LibraryRead::AiScope, Caller::User), Ok(12), "the user's own panel");
    assert!(read(LibraryRead::AiScope, local_ai).unwrap_err().contains("switched off"), "no AI while AI is off");
    assert!(
        s.reader.search("box", 10, LibraryRead::AiScope, Caller::User).unwrap().to_string().contains("lib/box.stl")
    );
    assert!(s.reader.search("box", 10, LibraryRead::AiScope, local_ai).is_err());
    config.ai.enabled = true;
    config.save_to(&s.config).unwrap();
    assert_eq!(read(LibraryRead::AiScope, local_ai), Ok(12));
    assert!(read(LibraryRead::AiScope, Caller::Ai(AccessClass::Cloud)).unwrap_err().contains("unknown id"));
}
