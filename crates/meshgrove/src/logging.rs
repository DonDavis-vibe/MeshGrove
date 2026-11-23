//! The log goes to stderr and to `meshgrove.log` in the cache folder: Windows release builds
//! have no console, and an app started from a menu shows its stderr nowhere either.

use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;

/// The main window's log.
pub const MAIN: &str = "meshgrove";
/// The log of a start that shows only model windows: it runs beside the main window, whose log
/// a start under the same name would move aside.
pub const VIEWER: &str = "meshgrove-viewer";

/// `meshgrove.log` in the cache folder; the previous start's log is kept as `meshgrove.old.log`.
pub fn log_file() -> Option<PathBuf> {
    named(MAIN)
}

fn named(name: &str) -> Option<PathBuf> {
    Some(crate::ui::settings::cache_dir()?.join(format!("{name}.log")))
}

/// Starts the logger (`RUST_LOG` as usual), writing to the log file `name` as well, and logs
/// panics, which would otherwise leave no trace without a console.
pub fn init(name: &str) {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,meshgrove=info"));
    // To the millisecond: what a start spends its time on shows between the lines.
    builder.format_timestamp_millis();
    if let Some(file) = named(name).and_then(|path| open(&path)) {
        builder.target(env_logger::Target::Pipe(Box::new(Tee(file))));
    }
    builder.init();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("{info}");
        default(info);
    }));
}

/// A fresh log file, the previous one moved aside.
fn open(path: &std::path::Path) -> Option<File> {
    std::fs::create_dir_all(path.parent()?).ok()?;
    let _ = std::fs::rename(path, path.with_extension("old.log"));
    File::create(path).ok()
}

/// Writes to stderr and to the log file.
struct Tee(File);

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        self.0.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_start_keeps_the_previous_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meshgrove").join("meshgrove.log");
        Tee(open(&path).unwrap()).write_all(b"first start\n").unwrap();
        Tee(open(&path).unwrap()).write_all(b"second start\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second start\n");
        assert_eq!(std::fs::read_to_string(path.with_file_name("meshgrove.old.log")).unwrap(), "first start\n");
        // The viewer's log keeps its own previous one and leaves the main window's alone.
        let viewer = path.with_file_name("meshgrove-viewer.log");
        Tee(open(&viewer).unwrap()).write_all(b"a model\n").unwrap();
        Tee(open(&viewer).unwrap()).write_all(b"another\n").unwrap();
        assert_eq!(std::fs::read_to_string(path.with_file_name("meshgrove-viewer.old.log")).unwrap(), "a model\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second start\n");
    }
}
