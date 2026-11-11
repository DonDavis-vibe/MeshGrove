//! The AI agents on this computer that the chat can use: the installed ones Meshgrove finds by
//! itself (Claude Code, Codex, Antigravity, OpenCode, …) and the ones the user added as a
//! command line. All of them speak the Agent Client Protocol (ACP, <https://agentclientprotocol.com>)
//! over their stdin and stdout. A desktop app often starts without the shell's PATH, so the usual
//! install folders are searched too. Detection never starts a login or a session: each agent found
//! is asked for `--version` (with a time limit), and whether it looks logged in comes only from
//! whether its own credentials file exists.
//!
//! The idea and the list of agents come from SolidCraft (`crates/ui-egui/src/agent_chat/discover.rs`
//! and `agents.rs`, MIT license; see `meshgrove_core::ai::agent_setup` for the notice).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use meshgrove_core::config::{AiConfig, ENDPOINT_ASSISTANT};

/// An agent Meshgrove knows how to start in ACP mode.
pub struct Known {
    pub id: &'static str,
    pub name: &'static str,
    /// Program names to look for, in order of preference.
    pub programs: &'static [&'static str],
    /// Arguments that put it in ACP mode.
    pub args: &'static [&'static str],
    /// Its one-line install hint.
    pub install: &'static str,
    /// Its own credentials file, relative to the home folder (`None`: can't tell), and the
    /// variable that moves its settings folder (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) with the file's
    /// name inside it.
    pub credentials: Option<&'static str>,
    pub config_dir_var: Option<(&'static str, &'static str)>,
    /// Variables holding a key it uses instead of a login.
    pub key_vars: &'static [&'static str],
    /// Whether `--version` prints its version. Antigravity's server prints its build date and takes
    /// seconds to start, so it is not asked.
    pub reports_version: bool,
}

/// The agents looked for. The first one found is the default when no endpoint is set up.
pub const KNOWN: &[Known] = &[
    Known {
        id: "claude",
        name: "Claude Code",
        programs: &["claude-agent-acp", "claude-code-acp"],
        args: &[],
        install: "npm install -g @agentclientprotocol/claude-agent-acp",
        credentials: Some(".claude/.credentials.json"),
        config_dir_var: Some(("CLAUDE_CONFIG_DIR", ".credentials.json")),
        key_vars: &["ANTHROPIC_API_KEY"],
        reports_version: true,
    },
    Known {
        id: "codex",
        name: "Codex",
        programs: &["codex-acp"],
        args: &[],
        install: "npm install -g @agentclientprotocol/codex-acp",
        credentials: Some(".codex/auth.json"),
        config_dir_var: Some(("CODEX_HOME", "auth.json")),
        key_vars: &["OPENAI_API_KEY", "CODEX_API_KEY"],
        reports_version: true,
    },
    // Google's `agy` command line has no ACP mode; its ACP server is a separate download that the
    // ACP registry lists (`agy_acp_server.par`, `agy_acp_server.exe` on Windows).
    Known {
        id: "antigravity",
        name: "Google Antigravity",
        programs: &["agy_acp_server.par", "agy_acp_server"],
        args: &[],
        install: "download the Antigravity ACP server (agentclientprotocol.com/registry) and put agy_acp_server.par on your PATH",
        credentials: Some(".gemini/antigravity-acp/acp_token.json"),
        config_dir_var: None,
        key_vars: &["GEMINI_API_KEY"],
        reports_version: false,
    },
    Known {
        id: "gemini",
        name: "Gemini CLI",
        programs: &["gemini"],
        args: &["--acp"],
        install: "npm install -g @google/gemini-cli",
        credentials: Some(".gemini/oauth_creds.json"),
        config_dir_var: None,
        key_vars: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        reports_version: true,
    },
    Known {
        id: "opencode",
        name: "OpenCode",
        programs: &["opencode"],
        args: &["acp"],
        install: "npm install -g opencode-ai",
        credentials: Some(".local/share/opencode/auth.json"),
        config_dir_var: None,
        key_vars: &[],
        reports_version: true,
    },
    Known {
        id: "grok",
        name: "XAI Grok",
        programs: &["grok"],
        args: &["agent", "stdio"],
        install: "see https://docs.x.ai/build/overview, then run grok login",
        credentials: Some(".grok/auth.json"),
        config_dir_var: None,
        key_vars: &["XAI_API_KEY"],
        reports_version: true,
    },
    Known {
        id: "copilot",
        name: "GitHub Copilot CLI",
        programs: &["copilot"],
        args: &["--acp"],
        install: "npm install -g @github/copilot",
        credentials: None,
        config_dir_var: None,
        key_vars: &["GH_TOKEN", "GITHUB_TOKEN"],
        reports_version: true,
    },
    Known {
        id: "qwen",
        name: "Qwen Code",
        programs: &["qwen"],
        args: &["--acp"],
        install: "npm install -g @qwen-code/qwen-code",
        credentials: Some(".qwen/oauth_creds.json"),
        config_dir_var: None,
        key_vars: &["DASHSCOPE_API_KEY"],
        reports_version: true,
    },
    Known {
        id: "goose",
        name: "goose",
        programs: &["goose"],
        args: &["acp"],
        install: "see block.github.io/goose",
        credentials: None,
        config_dir_var: None,
        key_vars: &[],
        reports_version: true,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Login {
    /// Its credentials file is there.
    LooksLoggedIn,
    /// No login file, but one of its key variables is set.
    KeyInEnvironment,
    /// Its credentials file is missing (it may still use a key from the environment).
    NotLoggedIn,
    /// It keeps no file we can look at (system keychain, or unknown).
    Unknown,
}

/// An agent found installed.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: &'static str,
    pub name: &'static str,
    /// The full command: the program's path and its ACP arguments.
    pub argv: Vec<String>,
    pub version: Option<String>,
    pub login: Login,
    /// Why its program doesn't start: asked for `--version`, it failed without printing one.
    pub problem: Option<String>,
    /// Its one-line install hint, to reinstall it with.
    pub install: &'static str,
}

/// What looking for the agents found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detection {
    pub found: Vec<Found>,
    pub took_ms: u64,
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).filter(|p| p.is_absolute())
}

/// PATH, then the usual places npm, Homebrew, nvm, Volta, bun, pipx and Grok put commands, and on
/// Windows Node's own install folders: a launcher can start Meshgrove with a PATH that lacks them,
/// and an npm agent's `.cmd` shim then can't find `node`.
pub fn search_dirs() -> Vec<PathBuf> {
    search_dirs_in(&|name| std::env::var_os(name), cfg!(windows))
}

fn search_dirs_in(var: &dyn Fn(&str) -> Option<OsString>, windows: bool) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = var("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(prefix) = var("NPM_CONFIG_PREFIX").map(PathBuf::from) {
        dirs.push(if windows { prefix.clone() } else { prefix.join("bin") });
    }
    if let Some(h) = var(if windows { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).filter(|p| p.is_absolute()) {
        for d in [".local/bin", ".npm-global/bin", ".npm-packages/bin", ".volta/bin", ".bun/bin", ".cargo/bin", ".grok/bin", "bin"]
        {
            dirs.push(h.join(d));
        }
        // nvm: every installed Node version's bin, the newest first.
        if let Ok(rd) = std::fs::read_dir(h.join(".nvm/versions/node")) {
            let mut v: Vec<PathBuf> = rd.flatten().take(64).map(|e| e.path().join("bin")).collect();
            v.sort();
            dirs.extend(v.into_iter().rev());
        }
    }
    if windows {
        if let Some(a) = var("APPDATA") {
            dirs.push(PathBuf::from(a).join("npm"));
        }
        for program_files in ["ProgramFiles", "ProgramFiles(x86)"] {
            dirs.extend(var(program_files).map(|d| PathBuf::from(d).join("nodejs")));
        }
        dirs.extend(var("NVM_SYMLINK").map(PathBuf::from));
    } else {
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/home/linuxbrew/.linuxbrew/bin", "/usr/bin"] {
            dirs.push(PathBuf::from(d));
        }
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| d.is_absolute() && seen.insert(d.clone()));
    dirs
}

/// The first `program` in `dirs` (on Windows with `.exe`/`.cmd`/`.bat`).
pub fn find_in(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let names = names_for(program, cfg!(windows));
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

/// The file names `program` can have. On Windows only ones it can run: npm puts a Unix shell
/// script with the bare name next to the `.cmd` shim, and starting that fails.
fn names_for(program: &str, windows: bool) -> Vec<String> {
    if windows {
        [".exe", ".cmd", ".bat"].iter().map(|e| format!("{program}{e}")).collect()
    } else {
        vec![program.to_string()]
    }
}

/// Start probes and sessions with the same PATH used for discovery: desktop launchers often omit
/// Node's install folder even when an adapter with a `#!/usr/bin/env node` shebang was found. Only
/// the child's environment changes.
pub(super) fn command_for(program: &Path) -> Command {
    let mut cmd = if cfg!(windows)
        && program.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
    {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    };
    if let Ok(path) = std::env::join_paths(search_dirs()) {
        cmd.env("PATH", path);
    }
    cmd
}

/// What `program --version` showed.
#[derive(Debug, Default, PartialEq)]
struct Probe {
    version: Option<String>,
    /// Why it can't start: the system refused to start it, or it failed and printed nothing but
    /// an error. A probe that runs out of time says nothing either way (a slow machine).
    problem: Option<String>,
}

/// How long an agent is given to answer `--version`.
const PROBE_LIMIT: Duration = Duration::from_secs(3);
/// The longest reason kept from a failed probe.
const PROBLEM_CHARS: usize = 300;

/// Ask `program --version`; it is stopped after `limit`.
fn probe(program: &Path, limit: Duration) -> Probe {
    let mut cmd = command_for(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: probing a console agent doesn't flash a terminal window.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = match cmd.arg("--version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return Probe {
                version: None,
                problem: Some(cut(&e.to_string().replace("%1", &program.display().to_string()))),
            };
        }
    };
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Probe::default();
            }
        }
    };
    let out = read_capped(child.stdout.take());
    if let Some(line) = out.lines().map(str::trim).find(|l| !l.is_empty()) {
        return Probe { version: Some(version_of(line)), problem: None };
    }
    if status.success() {
        return Probe::default();
    }
    let err = read_capped(child.stderr.take());
    let reason =
        err.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string).unwrap_or_else(|| status.to_string());
    Probe { version: None, problem: Some(cut(&reason)) }
}

/// What a finished probe wrote on a pipe, up to 64 KiB.
fn read_capped(pipe: Option<impl std::io::Read>) -> String {
    let mut bytes = Vec::new();
    if let Some(p) = pipe {
        let _ = std::io::Read::read_to_end(&mut p.take(64 * 1024), &mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn cut(text: &str) -> String {
    text.chars().take(PROBLEM_CHARS).collect()
}

/// The version number in a `--version` line (`codex-cli 0.162.1`, `2.1.295 (Claude Code)`,
/// `@scope/pkg 2.2.2`), else the line itself, cut short.
fn version_of(line: &str) -> String {
    let v = line
        .split_whitespace()
        .find(|w| w.trim_start_matches('v').starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(line);
    v.chars().take(40).collect()
}

/// Whether `k` looks signed in, from files and variables only: nothing is started or asked.
fn login_of(k: &Known, home: Option<&Path>) -> Login {
    let file = match k
        .config_dir_var
        .and_then(|(var, name)| std::env::var_os(var).filter(|v| !v.is_empty()).map(|d| PathBuf::from(d).join(name)))
    {
        Some(p) => Some(p),
        None => k.credentials.zip(home).map(|(rel, h)| h.join(rel)),
    };
    let key = || k.key_vars.iter().any(|v| std::env::var_os(v).is_some_and(|x| !x.is_empty()));
    match file {
        Some(f) if f.is_file() => Login::LooksLoggedIn,
        _ if key() => Login::KeyInEnvironment,
        // Claude Code keeps its login in the keychain on macOS: the file says nothing there.
        _ if cfg!(target_os = "macos") && k.id == "claude" => Login::Unknown,
        Some(_) => Login::NotLoggedIn,
        None => Login::Unknown,
    }
}

/// Look for every known agent. Blocking (each probe has a time limit): run it off the UI thread.
pub fn detect() -> Detection {
    detect_in(&search_dirs(), home().as_deref(), PROBE_LIMIT)
}

fn detect_in(dirs: &[PathBuf], home: Option<&Path>, limit: Duration) -> Detection {
    let t0 = Instant::now();
    let mut d = Detection::default();
    for k in KNOWN {
        let Some(path) = k.programs.iter().find_map(|p| find_in(p, dirs)) else { continue };
        let probe = if k.reports_version { probe(&path, limit) } else { Probe::default() };
        let mut argv = vec![path.to_string_lossy().into_owned()];
        argv.extend(k.args.iter().map(|a| (*a).to_string()));
        d.found.push(Found {
            id: k.id,
            name: k.name,
            argv,
            version: probe.version,
            login: login_of(k, home),
            problem: probe.problem,
            install: k.install,
        });
    }
    d.took_ms = t0.elapsed().as_millis() as u64;
    d
}

/// Split a command line into the program and its arguments: whitespace separates, double or
/// single quotes group, a backslash before a quote keeps it. No shell runs it.
pub fn split_command(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (_, '\\') if matches!(chars.peek(), Some('"') | Some('\'')) => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                started = true;
            }
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (Some(q), _) if c == q => quote = None,
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            _ => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

/// One way the chat can be answered, as the pickers show it: the endpoint, or an agent.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// An agent: what to start. Empty for the endpoint.
    pub argv: Vec<String>,
    /// An agent's model runs on this computer (a custom agent the user marked so); the endpoint
    /// as its own setting says.
    pub local: bool,
    /// Found installed (not added by the user, not the endpoint).
    pub found: bool,
    /// A short line under the name: the version, the model and address, or the command.
    pub detail: String,
    /// Found installed, but its program didn't start when asked its version: why.
    pub problem: Option<String>,
}

impl Entry {
    pub fn is_endpoint(&self) -> bool {
        self.id == ENDPOINT_ASSISTANT
    }
}

/// Every way the chat can be answered: the endpoint when set up, the agents found, then the ones
/// the user added.
pub fn entries(ai: &AiConfig, detection: Option<&Detection>) -> Vec<Entry> {
    let mut out = Vec::new();
    if let Some(e) = ai.endpoint.as_ref().filter(|e| !e.base_url.trim().is_empty()) {
        out.push(Entry {
            id: ENDPOINT_ASSISTANT.into(),
            name: t!("assistant.endpoint").into_owned(),
            argv: Vec::new(),
            local: e.local,
            found: false,
            detail: format!("{} · {}", e.model, e.base_url),
            problem: None,
        });
    }
    for f in detection.map(|d| d.found.as_slice()).unwrap_or_default() {
        let detail = match (&f.problem, &f.version) {
            (Some(p), _) => t!("assistant.cannot_start", error = p.clone()).into_owned(),
            (None, Some(v)) => v.clone(),
            (None, None) => short_command(&f.argv.join(" ")),
        };
        out.push(Entry {
            id: f.id.to_string(),
            name: f.name.to_string(),
            argv: f.argv.clone(),
            local: false,
            found: true,
            detail,
            problem: f.problem.clone(),
        });
    }
    for a in &ai.agents {
        out.push(Entry {
            id: a.id.clone(),
            name: if a.name.trim().is_empty() { a.id.clone() } else { a.name.clone() },
            argv: split_command(&a.command),
            local: a.local,
            found: false,
            detail: short_command(&a.command),
            problem: None,
        });
    }
    out
}

/// The entry the chat uses: the one picked in the settings, else the endpoint, else the first
/// agent found.
pub fn current(ai: &AiConfig, detection: Option<&Detection>) -> Option<Entry> {
    let all = entries(ai, detection);
    let pick = |id: &str| all.iter().find(|e| e.id == id).cloned();
    (!ai.assistant.is_empty()).then(|| pick(&ai.assistant)).flatten().or_else(|| all.into_iter().next())
}

/// A free id for an agent the user adds: `agent-1`, `agent-2`, …
pub fn new_id(ai: &AiConfig) -> String {
    (1..)
        .map(|n| format!("agent-{n}"))
        .find(|id| !ai.agents.iter().any(|a| &a.id == id) && !KNOWN.iter().any(|k| k.id == id))
        .unwrap_or_else(|| "agent".into())
}

/// A command line cut short for a label: the program's name and its arguments.
pub fn short_command(line: &str) -> String {
    let mut words = split_command(line);
    if let Some(first) = words.first_mut() {
        *first = Path::new(first).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| first.clone());
    }
    let text = words.join(" ");
    if text.chars().count() > 48 { format!("{}…", text.chars().take(47).collect::<String>()) } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_split_like_a_shell_without_one() {
        assert_eq!(split_command("gemini --acp"), ["gemini", "--acp"]);
        assert_eq!(
            split_command(r#"  "C:\Program Files\x\agent.exe"  --flag 'a b' "#),
            [r"C:\Program Files\x\agent.exe", "--flag", "a b"]
        );
        assert_eq!(split_command(r#"a "" b"#), ["a", "", "b"]);
        assert!(split_command("   ").is_empty());
        assert_eq!(short_command("/usr/local/bin/opencode acp"), "opencode acp");
    }

    #[test]
    fn versions_are_read_from_what_agents_print() {
        assert_eq!(version_of("codex-cli 0.162.1"), "0.162.1");
        assert_eq!(version_of("2.1.295 (Claude Code)"), "2.1.295");
        assert_eq!(version_of("v1.2.3"), "v1.2.3");
        assert_eq!(version_of("no number here"), "no number here");
    }

    #[test]
    fn the_usual_install_folders_are_searched_after_path() {
        let var = |name: &str| -> Option<OsString> {
            match name {
                "PATH" => Some("/usr/bin:/opt/x/bin".into()),
                "HOME" => Some("/home/u".into()),
                _ => None,
            }
        };
        let dirs = search_dirs_in(&var, false);
        assert_eq!(dirs[0], PathBuf::from("/usr/bin"));
        assert!(dirs.contains(&PathBuf::from("/home/u/.local/bin")));
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert_eq!(dirs.iter().filter(|d| d.as_path() == Path::new("/usr/bin")).count(), 1, "no doubles");
    }

    #[cfg(unix)]
    #[test]
    fn a_found_agent_is_asked_its_version_within_a_time_limit() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("codex-acp");
        std::fs::write(&fake, "#!/bin/sh\necho codex-cli 9.9.9\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let slow = dir.path().join("gemini");
        std::fs::write(&slow, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();
        let d = detect_in(&[dir.path().to_path_buf()], Some(dir.path()), Duration::from_millis(300));
        let codex = d.found.iter().find(|f| f.id == "codex").unwrap();
        assert_eq!(codex.version.as_deref(), Some("9.9.9"));
        assert_eq!(codex.argv[0], fake.to_string_lossy());
        assert_eq!(codex.login, Login::NotLoggedIn);
        let gemini = d.found.iter().find(|f| f.id == "gemini").unwrap();
        assert_eq!((gemini.version.as_deref(), gemini.problem.as_deref()), (None, None), "ran out of time: no verdict");
        assert_eq!(gemini.argv, [slow.to_string_lossy().to_string(), "--acp".to_string()]);
        assert!(!d.found.iter().any(|f| f.id == "claude"));
    }

    #[test]
    fn entries_put_the_endpoint_first_and_the_picked_one_current() {
        let mut ai = AiConfig::default();
        let detection = Detection {
            found: vec![Found {
                id: "claude",
                name: "Claude Code",
                argv: vec!["/x/claude-agent-acp".into()],
                version: Some("1.0".into()),
                login: Login::LooksLoggedIn,
                problem: None,
                install: "",
            }],
            took_ms: 0,
        };
        assert_eq!(current(&ai, Some(&detection)).map(|e| e.id), Some("claude".into()));
        ai.endpoint = Some(meshgrove_core::config::EndpointConfig { model: "qwen3".into(), ..Default::default() });
        assert_eq!(current(&ai, Some(&detection)).map(|e| e.id), Some(ENDPOINT_ASSISTANT.into()));
        ai.assistant = "claude".into();
        assert_eq!(current(&ai, Some(&detection)).map(|e| e.name), Some("Claude Code".into()));
        ai.assistant = "gone".into();
        assert!(current(&ai, Some(&detection)).unwrap().is_endpoint(), "a missing pick falls back");
        ai.agents.push(meshgrove_core::config::AgentConfig {
            id: new_id(&ai),
            name: String::new(),
            command: "my-agent --acp".into(),
            local: true,
        });
        let all = entries(&ai, Some(&detection));
        assert_eq!(all.len(), 3);
        assert_eq!((all[2].id.as_str(), all[2].name.as_str(), all[2].local), ("agent-1", "agent-1", true));
        assert_eq!(all[2].argv, ["my-agent", "--acp"]);
    }
}
