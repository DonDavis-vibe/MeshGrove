//! The one wasmtime engine of the process, its epoch clock, and the cache of compiled plugins.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use wasmtime::component::Component;
use wasmtime::{Config, Engine};

use crate::Error;

/// How often the epoch advances: the granularity of deadlines and of cancelling.
pub const TICK: Duration = Duration::from_millis(10);

static ENGINE: OnceLock<Engine> = OnceLock::new();

/// The engine every plugin runs on. Its first use starts the epoch clock (a thread that
/// advances the epoch every [`TICK`]), which interrupts plugins at their deadline.
pub fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.wasm_component_model(true).epoch_interruption(true).parallel_compilation(true);
        let engine = Engine::new(&config).expect("wasmtime accepts this configuration");
        let clock = engine.weak();
        std::thread::Builder::new()
            .name("plugin-epoch".into())
            .spawn(move || {
                while let Some(engine) = clock.upgrade() {
                    engine.increment_epoch();
                    drop(engine);
                    std::thread::sleep(TICK);
                }
            })
            .expect("a thread for the plugin clock");
        engine
    })
}

/// A compiled plugin.
pub struct Compiled {
    pub component: Component,
    /// blake3 of the `.wasm` file.
    pub hash: String,
    /// It came from the cache, not the compiler.
    pub from_cache: bool,
}

/// `<cache>/<blake3>-<engine>.cwasm`: the engine part changes with wasmtime's version and
/// settings, so an update never loads code compiled by another one.
fn cache_file(cache: &Path, hash: &str) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    engine().precompile_compatibility_hash().hash(&mut hasher);
    cache.join(format!("{hash}-{:016x}.cwasm", hasher.finish()))
}

/// Compiles the plugin `bytes`, or loads what an earlier run compiled from `cache`.
pub fn compile(bytes: &[u8], cache: Option<&Path>) -> Result<Compiled, Error> {
    let engine = engine();
    let hash = blake3::hash(bytes).to_hex().to_string();
    let file = cache.map(|c| cache_file(c, &hash));
    if let Some(file) = file.as_ref().filter(|f| f.is_file()) {
        // SAFETY: only Meshgrove writes this folder, with code this engine compiled (its
        // settings are in the name); wasmtime checks the header and refuses other code.
        match unsafe { Component::deserialize_file(engine, file) } {
            Ok(component) => return Ok(Compiled { component, hash, from_cache: true }),
            Err(e) => {
                log::warn!("plugin cache {}: {e}", file.display());
                let _ = std::fs::remove_file(file);
            }
        }
    }
    let component = Component::new(engine, bytes)
        .map_err(|e| Error::Load(format!("not a WebAssembly component for wasm32-wasip2: {e}")))?;
    if let Some(file) = &file {
        match component.serialize() {
            Ok(code) => {
                if let Err(e) = write_atomic(file, &code) {
                    log::warn!("plugin cache {}: {e}", file.display());
                }
            }
            Err(e) => log::warn!("plugin cache: {e}"),
        }
    }
    Ok(Compiled { component, hash, from_cache: false })
}

/// The version of the `meshgrove:plugin` interface the component exports, if any.
pub fn exported_api(component: &Component) -> Option<String> {
    let ty = component.component_type();
    ty.exports(engine()).find_map(|(name, _)| name.strip_prefix("meshgrove:plugin/plugin@").map(str::to_owned))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}
