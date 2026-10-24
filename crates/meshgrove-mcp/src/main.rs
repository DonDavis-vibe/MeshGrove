//! `meshgrove-mcp`: lets the user's own AI assistant (Claude Desktop, Claude Code, LM Studio,
//! Cursor, VS Code …) search the Meshgrove library and suggest tags and lists, over MCP on
//! stdin/stdout. Meshgrove brings no AI of its own, and nothing in the library changes until the
//! user accepts a suggestion in Meshgrove. Works while Meshgrove is closed (on the last index).
//!
//! `meshgrove-mcp [--local] [--client NAME]` — `--local` when the assistant's model runs on this
//! computer or network: it then also sees libraries opened to local AI only.
//! `meshgrove-mcp --print-config claude-desktop|claude-code|lm-studio|cursor|vscode [--local]`
//! prints what to paste into that client. Test runs take `--config`, `--db`, `--proposals`, `--thumbs`.

mod prompts;
mod server;

use std::path::PathBuf;
use std::process::ExitCode;

use meshgrove_core::ai::proposals::ProposalStore;
use meshgrove_core::ai::snippets::{self, Client};
use meshgrove_core::cache::ThumbCache;
use meshgrove_core::config::AccessClass;
use meshgrove_core::db::Library;
use rmcp::ServiceExt;

use crate::server::{Meshgrove, Paths};

const USAGE: &str = "usage: meshgrove-mcp [--local] [--client NAME]
       meshgrove-mcp --print-config claude-desktop|claude-code|lm-studio|cursor|vscode [--local]
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

fn main() -> ExitCode {
    // Logs go to stderr: stdout carries the protocol.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .target(env_logger::Target::Stderr)
        .init();
    let args = match parse(std::env::args().skip(1)) {
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
