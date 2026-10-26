//! `meshgrove-mcp`: lets the user's own AI assistant (Claude Desktop, Claude Code, LM Studio,
//! Cursor, VS Code …) search the Meshgrove library and suggest tags and lists, over MCP on
//! stdin/stdout. Meshgrove brings no AI of its own, and nothing in the library changes until the
//! user accepts a suggestion in Meshgrove. Works while Meshgrove is closed (on the last index).
//!
//! `meshgrove-mcp [--local] [--client NAME]` — `--local` when the assistant's model runs on this
//! computer or network: it then also sees libraries opened to local AI only.
//! `meshgrove-mcp --print-config claude-desktop|claude-code|lm-studio|cursor|vscode [--local]`
//! prints what to paste into that client. Test runs take `--config`, `--db`, `--proposals`, `--thumbs`.
//! `meshgrove-mcp setup|remove [--dry-run] [--client NAME]` adds this program to each installed
//! assistant, or takes it out again (what Meshgrove does by itself while AI is on).

mod prompts;
mod server;

use std::path::PathBuf;
use std::process::ExitCode;

use meshgrove_core::ai::agent_setup::{self, Change, Env, Mode, Server};
use meshgrove_core::ai::proposals::ProposalStore;
use meshgrove_core::ai::snippets::{self, Client};
use meshgrove_core::cache::ThumbCache;
use meshgrove_core::config::AccessClass;
use meshgrove_core::db::Library;
use rmcp::ServiceExt;

use crate::server::{Meshgrove, Paths};

const USAGE: &str = "usage: meshgrove-mcp [--local] [--client NAME]
       meshgrove-mcp --print-config claude-desktop|claude-code|lm-studio|cursor|vscode [--local]
       meshgrove-mcp setup|remove [--dry-run] [--client NAME]
Started by an AI assistant (MCP over stdin/stdout); see Meshgrove → Settings → AI & MCP.";

struct Args {
    class: AccessClass,
    client: Option<String>,
    paths: Paths,
    print_config: Option<String>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let defaults = || -> Option<(PathBuf, PathBuf, PathBuf)> {
        Some((Library::default_location()?, ProposalStore::default_location()?, ThumbCache::default_location()?))
    };
    let (db, proposals, thumbs) = defaults().ok_or("no data or cache folder")?;
    let mut out = Args {
        class: AccessClass::Cloud,
        client: None,
        paths: Paths { config: None, db, proposals, thumbs },
        print_config: None,
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--local" => out.class = AccessClass::Local,
            "--client" => out.client = Some(value()?),
            "--config" => out.paths.config = Some(value()?.into()),
            "--db" => out.paths.db = value()?.into(),
            "--proposals" => out.paths.proposals = value()?.into(),
            "--thumbs" => out.paths.thumbs = value()?.into(),
            "--print-config" => out.print_config = Some(value()?),
            "--version" | "-V" => {
                println!("meshgrove-mcp {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            // An AppImage passes its subcommand along.
            "mcp" => {}
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    Ok(Some(out))
}

fn print_config(name: &str, class: AccessClass) -> Result<(), String> {
    let client = Client::ALL.into_iter().find(|c| c.id() == name).ok_or_else(|| format!("unknown client {name}"))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    println!("{}", snippets::snippet(client, &exe, appimage.as_deref(), class));
    for file in client.config_files() {
        eprintln!("→ {}", file.display());
    }
    Ok(())
}

/// `setup` / `remove`: the `meshgrove` entry in each installed assistant, pointing at this
/// program (or the AppImage it came from). False when an assistant's settings could not be read.
fn setup(mode: Mode, args: &[String]) -> Result<bool, String> {
    let mut dry_run = false;
    let mut only = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--client" => only = Some(args.next().ok_or("--client needs a value")?.as_str()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    if let Some(id) = only
        && !agent_setup::CLIENTS.iter().any(|(c, _)| *c == id)
    {
        let ids: Vec<&str> = agent_setup::CLIENTS.iter().map(|(c, _)| *c).collect();
        return Err(format!("unknown client {id} (one of {})", ids.join(", ")));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    let outcomes = agent_setup::run(&Env::from_process(), &Server::new(&exe, appimage.as_deref()), mode, dry_run, only);
    if outcomes.is_empty() {
        println!("No AI assistant found on this computer.");
    }
    let (now, then) = if dry_run { ("would be ", "would stay ") } else { ("", "") };
    let mut ok = true;
    for o in &outcomes {
        let what = match &o.result {
            Ok(Some(Change::Added)) => format!("{now}connected"),
            Ok(Some(Change::Updated)) => format!("{now}updated to this program"),
            Ok(Some(Change::Unchanged)) => "already connected".to_owned(),
            Ok(Some(Change::Removed)) => format!("{now}removed"),
            Ok(Some(Change::Custom)) => format!("your own entry, {then}left as it is"),
            Ok(None) => "not connected".to_owned(),
            Err(e) => {
                ok = false;
                format!("left unchanged: {e}")
            }
        };
        println!("{:<16} {what}  ({})", o.name(), o.file.display());
    }
    if !dry_run
        && outcomes.iter().any(|o| matches!(o.result, Ok(Some(Change::Added | Change::Updated | Change::Removed))))
    {
        println!("Restart the assistants that changed.");
    }
    Ok(ok)
}

fn main() -> ExitCode {
    // Logs go to stderr: stdout carries the protocol.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .target(env_logger::Target::Stderr)
        .init();
    let argv: Vec<String> = std::env::args().skip(1).skip_while(|a| a == "mcp").collect();
    let mode = match argv.first().map(String::as_str) {
        Some("setup" | "install") => Some(Mode::Add),
        Some("remove" | "uninstall") => Some(Mode::Remove),
        _ => None,
    };
    if let Some(mode) = mode {
        return match setup(mode, &argv[1..]) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("meshgrove-mcp: {e}");
                ExitCode::from(2)
            }
        };
    }
    let args = match parse(argv.into_iter()) {
        Ok(Some(args)) => args,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("meshgrove-mcp: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(name) = &args.print_config {
        return match print_config(name, args.class) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("meshgrove-mcp: {e}");
                ExitCode::from(2)
            }
        };
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("meshgrove-mcp: {e}");
            return ExitCode::FAILURE;
        }
    };
    let server = Meshgrove::new(args.paths, args.class, args.client);
    let result = runtime.block_on(async move {
        let running = server.serve(rmcp::transport::stdio()).await.map_err(|e| e.to_string())?;
        running.waiting().await.map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}
