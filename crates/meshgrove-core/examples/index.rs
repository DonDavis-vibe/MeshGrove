//! Index a library root into a database and optionally search it.
//!
//!     cargo run --release -p meshgrove-core --example index -- library.db /path/to/root ["query"]

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use meshgrove_core::{db::Library, query, scan};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (db, root) = (Path::new(&args[0]), Path::new(&args[1]));
    let mut lib = Library::open(db).expect("open database");
    let t = Instant::now();
    let report = scan::scan(&mut lib, root, root, false, &AtomicBool::new(false)).expect("scan");
    println!(
        "scan: {} folders visited, {} listed, {:?}; {:?}",
        report.dirs_visited,
        report.changed_dirs.len(),
        t.elapsed(),
        lib.counts().unwrap()
    );
    if let Some(q) = args.get(2) {
        let t = Instant::now();
        let hits = lib.search(&query::parse(q), 1000).unwrap();
        println!("search {q:?}: {} hits in {:?}", hits.len(), t.elapsed());
        for e in hits.iter().take(5) {
            println!("  {}", e.path.display());
        }
    }
}
