//! What one plugin instance carries in its store: its WASI sandbox, memory limit, deadline,
//! cancel flag and the rights it was given; and the host functions it imports.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use meshgrove_plugin_api::HostInfo;
use wasmtime::component::ResourceTable;
use wasmtime::{ResourceLimiter, Store, UpdateDeadline};
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::Error;
use crate::bindings::meshgrove::plugin::{host, library, types};
use crate::guest_files::{self, GuestDirs};
use crate::library::{self as lib, Reading, Through};

/// Stops a call or job: the plugin sees `host.cancelled()`, and is interrupted at the next tick
/// if it does not look.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    /// A token set by whoever holds `flag` (the chat's stop button, a cancelled MCP request).
    pub fn from_flag(flag: Arc<AtomicBool>) -> CancelToken {
        CancelToken(flag)
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Why the host stopped a plugin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Interrupt {
    Cancelled,
    Timeout(Duration),
}

/// Memory and tables an instance may grow to.
pub(crate) struct Limits {
    pub memory_mb: u32,
    /// Growing memory past the limit was refused.
    pub hit: bool,
}

impl ResourceLimiter for Limits {
    fn memory_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        if desired > (self.memory_mb as usize) << 20 {
            self.hit = true;
            return Err(wasmtime::format_err!("memory limit of {} MiB reached", self.memory_mb));
        }
        Ok(true)
    }

    fn table_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        Ok(desired <= 1_000_000)
    }
}

/// Reports a call's or job's progress (fraction, text).
pub type Progress = Arc<dyn Fn(f32, &str) + Send + Sync>;

/// A folder only this store uses (its `/tmp`), removed with it.
struct TempFolder(PathBuf);

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    pub limits: Limits,
    pub plugin: String,
    /// How it reads the library: for whom, with which grant, and what the user opened with it.
    pub reading: Reading,
    pub info: HostInfo,
    pub cancel: CancelToken,
    /// When the current call must be done, and how long it had.
    pub deadline: Option<(Instant, Duration)>,
    pub interrupted: Option<Interrupt>,
    pub progress: Option<Progress>,
    /// Where `/data` and `/tmp` are on this computer.
    dirs: GuestDirs,
    _tmp: TempFolder,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

/// The folders an instance sees.
pub(crate) struct Folders<'a> {
    /// `/data`: the plugin's own folder.
    pub data: &'a Path,
    pub data_writable: bool,
    /// `/out`: the staging folder.
    pub out: &'a Path,
    /// Below this a fresh `/tmp` is made (and removed with the store).
    pub tmp_base: &'a Path,
    /// `tmp_base` is the `/tmp` itself (a panel's, which the host copies picked files into).
    pub tmp_exact: bool,
    /// For what the host puts on disk for the plugin without showing it ([`GuestDirs::scratch`]).
    pub scratch: &'a Path,
}

/// What a store is for: who, with which rights, seeing what.
pub(crate) struct Setup<'a> {
    pub reading: Reading,
    pub info: &'a HostInfo,
    pub memory_mb: u32,
    pub folders: Folders<'a>,
    pub cancel: CancelToken,
    pub progress: Option<Progress>,
}

pub(crate) fn store(setup: Setup) -> Result<Store<State>, Error> {
    let folders = &setup.folders;
    let tmp = match folders.tmp_exact {
        true => folders.tmp_base.to_path_buf(),
        false => folders.tmp_base.join(crate::staging::new_uid()),
    };
    for dir in [folders.data, folders.out, tmp.as_path()] {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = TempFolder(tmp);
    let data_perms = if folders.data_writable { FsPerms::ReadWrite } else { FsPerms::ReadOnly };
    let wasi = (|| -> wasmtime::Result<WasiCtx> {
        let mut b = WasiCtxBuilder::new();
        // No network, no environment; stderr for panics and debug output (stdout may be a
        // protocol, as in meshgrove-plugin-run's MCP server).
        b.allow_tcp(false).allow_udp(false).allow_ip_name_lookup(false).inherit_stderr();
        b.allow_blocking_current_thread(true);
        b.preopened_dir(folders.data, "/data", data_perms)?;
        b.preopened_dir(folders.out, "/out", FsPerms::ReadWrite)?;
        b.preopened_dir(&tmp.0, "/tmp", FsPerms::ReadWrite)?;
        Ok(b.build())
    })()
    .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
    let state = State {
        wasi,
        table: ResourceTable::new(),
        limits: Limits { memory_mb: setup.memory_mb, hit: false },
        plugin: setup.reading.plugin.clone(),
        reading: setup.reading,
        info: setup.info.clone(),
        cancel: setup.cancel,
        deadline: None,
        interrupted: None,
        progress: setup.progress,
        dirs: GuestDirs {
            data: folders.data.to_path_buf(),
            tmp: Some(tmp.0.clone()),
            scratch: folders.scratch.to_path_buf(),
        },
        _tmp: tmp,
    };
    let mut store = Store::new(crate::engine::engine(), state);
    store.limiter(|s| &mut s.limits);
    // Every tick: stop when cancelled or past the deadline.
    store.epoch_deadline_callback(|mut ctx| {
        let s = ctx.data_mut();
        if s.cancel.is_cancelled() {
            s.interrupted = Some(Interrupt::Cancelled);
            return Err(wasmtime::format_err!("cancelled"));
        }
        if let Some((at, limit)) = s.deadline
            && Instant::now() >= at
        {
            s.interrupted = Some(Interrupt::Timeout(limit));
            return Err(wasmtime::format_err!("deadline of {limit:?} passed"));
        }
        Ok(UpdateDeadline::Continue(1))
    });
    store.set_epoch_deadline(1);
    Ok(store)
}

/// Gets the store ready for the next call: `limit` from now (none: until cancelled).
pub(crate) fn arm(store: &mut Store<State>, limit: Option<Duration>) {
    let s = store.data_mut();
    s.deadline = limit.map(|l| (Instant::now() + l, l));
    s.interrupted = None;
    store.set_epoch_deadline(1);
}

/// What became of a failed call, as the host saw it.
pub(crate) fn failure(store: &Store<State>, error: &wasmtime::Error) -> Error {
    let s = store.data();
    match s.interrupted {
        Some(Interrupt::Cancelled) => Error::Cancelled,
        Some(Interrupt::Timeout(limit)) => Error::Timeout(limit),
        None if s.limits.hit => Error::Memory(s.limits.memory_mb),
        None => Error::Trap(error.to_string()),
    }
}

impl types::Host for State {}

impl host::Host for State {
    fn log(&mut self, level: host::Level, message: String) {
        let level = match level {
            host::Level::Error => log::Level::Error,
            host::Level::Warn => log::Level::Warn,
            host::Level::Info => log::Level::Info,
            host::Level::Debug => log::Level::Debug,
        };
        let message: String = message.chars().take(2000).collect();
        log::log!(target: "plugin", level, "{}: {message}", self.plugin);
    }

    fn progress(&mut self, fraction: f32, message: String) {
        if let Some(progress) = &self.progress {
            let message: String = message.chars().take(200).collect();
            progress(if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 }, &message);
        }
    }

    fn cancelled(&mut self) -> bool {
        self.cancel.is_cancelled()
    }

    fn info(&mut self) -> String {
        serde_json::to_string(&self.info).unwrap_or_default()
    }
}

/// Facts of a file the plugin reads only because the user opened it: without what they say
/// about other files (its copies, the G-code beside it, the folder's readme).
fn only_this_file(mut facts: serde_json::Value) -> serde_json::Value {
    if let Some(map) = facts.as_object_mut() {
        for key in ["copies", "related_gcode", "folder_readme_untrusted"] {
            map.remove(key);
        }
    }
    facts
}

impl State {
    /// Runs `f` with the plugin's clock stopped: the time the host takes to read the library or
    /// a file for it is the host's, not the plugin's, so the deadline moves on by as much.
    fn on_host<T>(&mut self, f: impl FnOnce(&Reading, &GuestDirs) -> Result<T, String>) -> Result<T, String> {
        let started = Instant::now();
        let result = f(&self.reading, &self.dirs);
        if let Some((until, limit)) = &mut self.deadline {
            *until += started.elapsed();
            *limit += started.elapsed();
        }
        result
    }
}

impl library::Host for State {
    fn search(&mut self, query: String, limit: u32) -> Result<String, String> {
        self.on_host(|reading, _| reading.all(|l, read, caller| l.search(&query, limit.min(500), read, caller)))
            .map(|v| v.to_string())
    }

    fn describe(&mut self, reference: String) -> Result<String, String> {
        // A part is described with the file it is in.
        let (reference, _) = lib::split_part(&reference)?;
        if guest_files::is_guest_path(reference) {
            let facts = self.on_host(|_, dirs| {
                let bytes = dirs.read(reference)?;
                let name = guest_files::file_name(reference);
                let mut facts = lib::mesh_facts(name, bytes.len(), &dirs.load(reference)?);
                if meshgrove_core::Kind::from_file_name(name) == meshgrove_core::Kind::ThreeMf {
                    facts["parts"] = lib::parts_json(&bytes, reference).into();
                }
                Ok(facts)
            })?;
            return Ok(facts.to_string());
        }
        let (facts, through) =
            self.on_host(|reading, _| reading.file(reference, |l, read, caller| l.describe(reference, read, caller)))?;
        Ok(if through == Through::Opening { only_this_file(facts) } else { facts }.to_string())
    }

    fn triangles(&mut self, reference: String, max_triangles: u32) -> Result<types::MeshData, String> {
        let max = max_triangles.clamp(1, lib::MAX_TRIANGLES);
        let (reference, part) = lib::split_part(&reference)?;
        let mesh = self.on_host(|reading, dirs| {
            if !guest_files::is_guest_path(reference) {
                let read = |l: &dyn lib::LibraryAccess, read, caller| l.triangles(reference, part, max, read, caller);
                return reading.file(reference, read).map(|(mesh, _)| mesh);
            }
            let mesh = match part {
                Some(part) => lib::part_mesh(&dirs.read(reference)?, part)?,
                None => dirs.load(reference)?,
            };
            let data = lib::simplify(&mesh, max as usize);
            if data.triangles() == 0 {
                return Err(format!("{reference} has no triangles"));
            }
            Ok(data)
        })?;
        mesh.check()?;
        Ok(types::MeshData { positions: mesh.positions, indices: mesh.indices })
    }
}
