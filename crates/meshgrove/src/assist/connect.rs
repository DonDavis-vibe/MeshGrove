//! Makes Meshgrove available to the AI assistants on this computer without a step from the
//! user: while AI is on and "Make Meshgrove available to AI apps" is checked, the `meshgrove`
//! entry is added to each installed assistant (`meshgrove_core::ai::agent_setup`), off the window
//! thread; the assistants that got it for the first time are named once. Turning either off takes
//! the entries out again. Best effort: nothing here can fail the app.
//!
//! The idea and its rules come from SolidCraft (`crates/ui-egui/src/ai_connect.rs`, MIT license;
//! see `meshgrove_core::ai::agent_setup` for the notice).

use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, bounded};
use meshgrove_core::ai::agent_setup::{self, Change, Env, Mode, Outcome, Server};

/// What the app keeps between frames.
#[derive(Default)]
pub struct AiConnect {
    /// This launch may touch the assistants' files: not a test or screenshot run, not a build
    /// from a source tree (that would point the user's assistants at a development binary).
    armed: bool,
    /// The wish as last acted on.
    applied: Option<bool>,
    job: Option<(Mode, Receiver<Vec<Outcome>>)>,
}

impl AiConnect {
    pub fn new(armed: bool) -> AiConnect {
        AiConnect { armed, ..AiConnect::default() }
    }

    /// Whether this launch may manage the assistants' files at all.
    pub fn allowed() -> bool {
        std::env::var_os("MESHGROVE_NO_AI_CONNECT").is_none()
            && !std::env::current_exe().is_ok_and(|exe| in_source_tree(&exe))
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Every frame: start a run when the wish differs from what was last done. Returns the
    /// assistants that were connected for the first time, once, when a run has finished.
    pub fn tick(&mut self, want: bool) -> Option<Vec<String>> {
        if !self.armed {
            return None;
        }
        if self.job.is_none() && self.applied != Some(want) {
            // At the first frame, off means nothing to do: entries go when it is turned off.
            let first = self.applied.is_none();
            self.applied = Some(want);
            if want || !first {
                self.job = spawn(if want { Mode::Add } else { Mode::Remove });
            }
        }
        let (mode, rx) = self.job.as_ref()?;
        let outcomes = rx.try_recv().ok()?;
        let mode = *mode;
        self.job = None;
        let file = announced_file()?;
        if mode == Mode::Remove {
            // Turned off: the next time it is on, the assistants are news again.
            let _ = std::fs::remove_file(file);
            return None;
        }
        let added: Vec<&str> = outcomes
            .iter()
            .filter(|o| matches!(o.result, Ok(Some(Change::Added | Change::Unchanged | Change::Updated))))
            .map(|o| o.client)
            .collect();
        let announced: Vec<String> =
            std::fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let fresh: Vec<&str> = added.into_iter().filter(|c| !announced.iter().any(|a| a == c)).collect();
        if fresh.is_empty() {
            return None;
        }
        let mut all = announced;
        all.extend(fresh.iter().map(|c| (*c).to_owned()));
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&file, serde_json::to_string(&all).unwrap_or_default());
        Some(fresh.iter().map(|c| agent_setup::client_name(c).to_owned()).collect())
    }

    /// "Connect now" in the settings: adds the entries at once, whatever was done before.
    pub fn connect_now(&mut self) {
        if self.job.is_none() {
            self.job = spawn(Mode::Add);
        }
    }

    /// A run is going on (the settings show a spinner).
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }
}

/// The server entry to write: `meshgrove-mcp` next to this program (or this AppImage).
pub fn server() -> Option<Server> {
    let (exe, appimage) = crate::ui::settings_ai::mcp_binary();
    if appimage.is_none() && !exe.exists() {
        return None;
    }
    let exe = exe.canonicalize().unwrap_or(exe);
    Some(Server::new(&exe, appimage.as_deref()))
}

/// What connecting would do for each assistant found, for the settings page. Reads the files,
/// writes nothing.
pub fn status() -> Vec<Outcome> {
    match server() {
        Some(server) => agent_setup::run(&Env::from_process(), &server, Mode::Add, true, None),
        None => Vec::new(),
    }
}

fn spawn(mode: Mode) -> Option<(Mode, Receiver<Vec<Outcome>>)> {
    let server = server()?;
    let (tx, rx) = bounded(1);
    std::thread::Builder::new()
        .name("ai-connect".into())
        .spawn(move || {
            let outcomes = agent_setup::run(&Env::from_process(), &server, mode, false, None);
            for o in &outcomes {
                match &o.result {
                    Ok(Some(change)) => log::info!("AI assistant {}: {change:?} ({})", o.name(), o.file.display()),
                    Ok(None) => {}
                    Err(e) => log::warn!("AI assistant {}: {} left unchanged: {e}", o.name(), o.file.display()),
                }
            }
            let _ = tx.send(outcomes);
        })
        .ok()?;
    Some((mode, rx))
}

/// The assistants already announced, so each one is named once.
fn announced_file() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("meshgrove").join("ai-apps-connected.json"))
}

/// A program built in a source tree (`…/target/debug/meshgrove`, `…/target/release/…`).
fn in_source_tree(exe: &Path) -> bool {
    let parts: Vec<String> = exe.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect();
    parts.windows(2).any(|w| w[0] == "target" && matches!(w[1].as_str(), "debug" | "release"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_from_a_source_tree_never_touch_the_assistants() {
        assert!(in_source_tree(Path::new("/home/u/src/meshgrove/target/release/meshgrove")));
        assert!(in_source_tree(Path::new("/home/u/src/meshgrove/target/debug/meshgrove")));
        assert!(!in_source_tree(Path::new("/home/u/.local/bin/meshgrove")));
        assert!(!in_source_tree(Path::new("/usr/bin/meshgrove")));
        // Unarmed (tests, screenshots), nothing ever runs.
        let mut c = AiConnect::new(false);
        assert_eq!(c.tick(true), None);
        assert!(!c.busy());
    }
}
