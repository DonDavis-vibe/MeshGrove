//! meshgrove-plugin-run: runs a Meshgrove plugin without Meshgrove — for plugin authors, scripts
//! and other programs that want the same `.wasm`.
//!
//!   meshgrove-plugin-run <plugin.wasm> manifest
//!   meshgrove-plugin-run <plugin.wasm> tools
//!   meshgrove-plugin-run <plugin.wasm> call <tool> ['<json>']   files land in --out
//!   meshgrove-plugin-run <plugin.wasm> job <kind> ['<json>']    files land in --out
//!   meshgrove-plugin-run <plugin.wasm> open <file>
//!   meshgrove-plugin-run <plugin.wasm> mcp                      its tools as an MCP server (stdio)
//!
//! Options: `--out <dir>` (default `./plugin-out`), `--library <dir>`, `--data <dir>`,
//! `--memory <MiB>`, `--no-cache`.
//! The plugin may offer tools and hand over bundles here (the bundle is printed, its files stay
//! in the output folder). With `--library` it reads that folder as a library, all of it, by ids
//! that start with the folder's name (`models/Garten/rose.stl`); without, it has none. `--data`
//! is the folder it sees as `/data` (by default its own in Meshgrove's plugins folder).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use base64::Engine;
use meshgrove_core::config::AccessClass;
use meshgrove_plugins::folder::FolderLibrary;
use meshgrove_plugins::{CancelToken, LibraryRead, Plugin, PluginGrants, Settings, Staging, ToolOutcome};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};

const USAGE: &str = "usage: meshgrove-plugin-run <plugin.wasm> (manifest | tools | call <tool> [json] | job <kind> [json] | open <file> | mcp)
                            [--out <dir>] [--library <dir>] [--data <dir>] [--memory <MiB>] [--no-cache]";

#[derive(Debug, PartialEq)]
struct Args {
    wasm: PathBuf,
    command: Vec<String>,
    out: PathBuf,
    /// A folder the plugin reads as a library.
    library: Option<PathBuf>,
    /// The folder the plugin sees as `/data`.
    data: Option<PathBuf>,
    memory_mb: Option<u32>,
    cache: bool,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut positional = Vec::new();
    let (mut out, mut memory_mb, mut cache) = (PathBuf::from("plugin-out"), None, true);
    let (mut library, mut data) = (None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next().ok_or("--out needs a folder")?.into(),
            "--library" => library = Some(args.next().ok_or("--library needs a folder")?.into()),
            "--data" => data = Some(args.next().ok_or("--data needs a folder")?.into()),
            "--memory" => {
                memory_mb = Some(args.next().and_then(|m| m.parse().ok()).ok_or("--memory needs MiB")?);
            }
            "--no-cache" => cache = false,
            "-h" | "--help" => return Err(USAGE.into()),
            _ => positional.push(arg),
        }
    }
    if positional.len() < 2 {
        return Err(USAGE.into());
    }
    let wasm = positional.remove(0).into();
    Ok(Args { wasm, command: positional, out, library, data, memory_mb, cache })
}

/// How the plugin runs here: its tools and bundles allowed; with `--library`, that folder to
/// read, all of it.
fn settings(args: &Args, base: &Path) -> Result<Settings, String> {
    let mut settings = Settings {
        grants: PluginGrants { ai_tools: true, bundles: true, ..PluginGrants::default() },
        memory_mb: args.memory_mb,
        cache: args.cache,
        ..Settings::new(base)
    };
    if let Some(dir) = &args.library {
        let library = FolderLibrary::new(dir).map_err(|e| format!("--library {}: {e}", dir.display()))?;
        settings.library = Some(Arc::new(library));
        settings.grants.library_read = LibraryRead::All;
    }
    if let Some(dir) = &args.data {
        std::fs::create_dir_all(dir).map_err(|e| format!("--data {}: {e}", dir.display()))?;
        settings.data = Some(dir.canonicalize().map_err(|e| format!("--data {}: {e}", dir.display()))?);
    }
    Ok(settings)
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("meshgrove-plugin-run: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    let base = meshgrove_plugins::default_base().ok_or("no data folder")?;
    let plugin =
        Plugin::load(&args.wasm, settings(&args, &base)?).map_err(|e| format!("{}: {e}", args.wasm.display()))?;
    let arg = |i: usize| args.command.get(i).map(String::as_str);
    let json_arg = |i: usize| -> Result<Value, String> {
        arg(i).map_or(Ok(json!({})), |text| serde_json::from_str(text).map_err(|e| format!("arguments: {e}")))
    };
    let print = |value: &Value| println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
    match (arg(0), arg(1)) {
        (Some("manifest"), _) => print(&serde_json::to_value(plugin.manifest()).unwrap_or_default()),
        (Some("tools"), _) => print(&serde_json::to_value(plugin.tools()).unwrap_or_default()),
        (Some("call"), Some(tool)) => {
            let staging = Staging::at(&args.out).map_err(|e| e.to_string())?;
            let outcome = plugin
                .call_tool_in(tool, &json_arg(2)?, staging, AccessClass::Local, &CancelToken::new())
                .map_err(|e| e.to_string())?;
            print(&outcome_json(&outcome, &args.out));
        }
        (Some("job"), Some(kind)) => {
            std::fs::create_dir_all(&args.out).map_err(|e| e.to_string())?;
            let progress: meshgrove_plugins::Progress =
                Arc::new(|fraction, message: &str| eprintln!("{:3.0} % {message}", fraction * 100.0));
            let result = plugin
                .run_job(kind, &json_arg(2)?, &args.out, &CancelToken::new(), Some(progress))
                .map_err(|e| e.to_string())?;
            print(&result);
        }
        (Some("open"), Some(file)) => {
            let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            let name = Path::new(file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let effects = plugin.open_file(&name, &bytes).map_err(|e| e.to_string())?;
            print(&serde_json::to_value(effects).unwrap_or_default());
        }
        (Some("mcp"), _) => serve(Arc::new(plugin), args.out)?,
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

/// A tool's answer as JSON: the result, the bundle and where its files are. A bundle that
/// puts its files beside library files also says where they would land.
fn outcome_json(outcome: &ToolOutcome, out: &Path) -> Value {
    let mut value = json!({"result": outcome.result, "images": outcome.images.len()});
    if let Some(staged) = &outcome.bundle {
        value["bundle"] = serde_json::to_value(&staged.bundle).unwrap_or_default();
        value["files_in"] = json!(out.display().to_string());
        if staged.bundle.uses_next_to() {
            value["would_land"] = match staged.resolve(Path::new(""), Path::new("")) {
                Ok(resolved) => resolved.files.iter().filter_map(|f| f.at.as_ref()).map(|p| json!(p)).collect(),
                Err(e) => json!(e.to_string()),
            };
        }
    }
    value
}

/// The plugin's tools over MCP on stdin/stdout, named `<plugin>_<tool>` as in Meshgrove.
struct Runner {
    plugin: Arc<Plugin>,
    out: PathBuf,
}

impl Runner {
    fn full_name(&self, tool: &str) -> String {
        format!("{}_{tool}", self.plugin.id())
    }
}

impl ServerHandler for Runner {
    fn get_info(&self) -> ServerConfig {
        let m = self.plugin.manifest();
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(format!("meshgrove-plugin-{}", m.id), m.version.clone()))
            .with_instructions(m.description.clone())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = self
            .plugin
            .tools()
            .iter()
            .map(|t| {
                let schema = t.input_schema.as_object().cloned().unwrap_or_default();
                Tool::new(self.full_name(&t.name), t.description.clone(), schema)
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let prefix = format!("{}_", self.plugin.id());
        let Some(tool) = request.name.strip_prefix(&prefix).map(str::to_owned) else {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!("unknown tool {}", request.name))]).into());
        };
        let args = Value::Object(request.arguments.unwrap_or_default());
        let (plugin, out) = (self.plugin.clone(), self.out.clone());
        let outcome = tokio::task::spawn_blocking(move || {
            let staging = Staging::at(&out).map_err(meshgrove_plugins::Error::from)?;
            plugin.call_tool_in(&tool, &args, staging, AccessClass::Local, &CancelToken::new()).map(|o| (o, out))
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(match outcome {
            Ok((outcome, out)) => {
                let mut content = vec![ContentBlock::text(outcome_json(&outcome, &out).to_string())];
                for image in &outcome.images {
                    let data = base64::engine::general_purpose::STANDARD.encode(&image.data);
                    content.push(ContentBlock::image(data, image.mime.clone()));
                }
                CallToolResult::success(content)
            }
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
        }
        .into())
    }
}

fn serve(plugin: Arc<Plugin>, out: PathBuf) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let running = Runner { plugin, out }.serve(rmcp::transport::stdio()).await.map_err(|e| e.to_string())?;
        running.waiting().await.map_err(|e| e.to_string())?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Result<Args, String> {
        parse_args(line.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn arguments_are_read() {
        let a = args("box.wasm call measure {} --library models --data scratch/data --out o --memory 64 --no-cache")
            .unwrap();
        assert_eq!(a.wasm, PathBuf::from("box.wasm"));
        assert_eq!(a.command, ["call", "measure", "{}"]);
        assert_eq!((a.library, a.data), (Some("models".into()), Some("scratch/data".into())));
        assert_eq!((a.out, a.memory_mb, a.cache), (PathBuf::from("o"), Some(64), false));
        let plain = args("box.wasm tools").unwrap();
        assert_eq!(
            (plain.library, plain.data, plain.out, plain.cache),
            (None, None, PathBuf::from("plugin-out"), true)
        );
        for bad in ["box.wasm", "box.wasm tools --library", "box.wasm tools --data", "--help"] {
            assert!(args(bad).is_err(), "{bad}");
        }
    }

    /// `--library` gives the plugin a folder to read, all of it; `--data` its own folder.
    #[test]
    fn a_folder_serves_as_the_library_and_another_as_data() {
        let dir = tempfile::tempdir().unwrap();
        let (models, data) = (dir.path().join("models"), dir.path().join("data"));
        std::fs::create_dir_all(&models).unwrap();
        let mut a = args("box.wasm tools").unwrap();
        let none = settings(&a, dir.path()).unwrap();
        assert!(none.library.is_none() && none.data.is_none());
        assert_eq!(none.grants.library_read, LibraryRead::None, "no library, nothing to read");
        (a.library, a.data) = (Some(models), Some(data.clone()));
        let with = settings(&a, dir.path()).unwrap();
        assert!(with.library.is_some() && with.grants.library_read == LibraryRead::All);
        assert_eq!(with.data, Some(data.canonicalize().unwrap()), "made if it was missing");
        assert!(with.grants.ai_tools && with.grants.bundles);
        a.library = Some(dir.path().join("missing"));
        assert!(settings(&a, dir.path()).is_err_and(|e| e.contains("--library")));
    }
}
