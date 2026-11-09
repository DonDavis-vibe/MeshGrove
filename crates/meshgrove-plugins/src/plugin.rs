//! A loaded plugin (compiled, checked, with its manifest and tools) and its calls.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use meshgrove_core::bundle::{Limits, ResolvedBundle, Source};
use meshgrove_core::config::{AccessClass, Config, LibraryRead, PluginConfig, PluginGrants};
use meshgrove_plugin_api::ui::Effect;
use meshgrove_plugin_api::{API_VERSION, Bundle, HostInfo, Manifest, Permission, ToolDef, WIT_VERSION};
use serde_json::Value;
use wasmtime::Store;
use wasmtime::component::{HasSelf, Linker};

use crate::bindings::{MeshgrovePlugin, MeshgrovePluginPre, PluginError};
use crate::guest_files::GuestDirs;
use crate::library::{Caller, LibraryAccess, Opened, Reading};
use crate::staging::{Staging, new_uid};
use crate::state::{self, CancelToken, Folders, Progress, Setup, State};
use crate::{DEFAULT_MEMORY_MB, EVENT_DEADLINE, Error, TOOL_DEADLINE};

/// Largest answer of a tool, as JSON.
const MAX_RESULT: usize = 1 << 20;
/// Most pictures with one answer, and the largest.
const MAX_IMAGES: usize = 4;
const MAX_IMAGE: usize = 8 << 20;

/// How a plugin runs here.
#[derive(Clone)]
pub struct Settings {
    pub grants: PluginGrants,
    /// Memory of one instance in MiB; `None`: what the manifest asks for, else
    /// [`DEFAULT_MEMORY_MB`].
    pub memory_mb: Option<u32>,
    /// The plugins folder ([`crate::default_base`]): data, staging and temporary folders of
    /// each plugin below `<id>/`, compiled code in `.cache/`.
    pub base: PathBuf,
    /// Keep compiled code in `<base>/.cache`.
    pub cache: bool,
    /// The plugin's own folder (`/data`); `None`: `<base>/<id>/data`.
    pub data: Option<PathBuf>,
    pub library: Option<Arc<dyn LibraryAccess>>,
    pub info: HostInfo,
    /// How long a tool call may take ([`TOOL_DEADLINE`]).
    pub tool_deadline: Duration,
    /// How long a panel may take for one event ([`EVENT_DEADLINE`]).
    pub event_deadline: Duration,
    /// The plugin API this host offers ([`API_VERSION`]; tests pretend to be another). The
    /// manifest is checked against it; the world the plugin exports against [`WIT_VERSION`].
    pub host_api: String,
}

impl Settings {
    /// Nothing granted, no library.
    pub fn new(base: &Path) -> Settings {
        Settings {
            grants: PluginGrants::default(),
            memory_mb: None,
            base: base.to_path_buf(),
            cache: true,
            data: None,
            library: None,
            info: HostInfo {
                language: "en".into(),
                meshgrove: env!("CARGO_PKG_VERSION").into(),
                api: API_VERSION.into(),
                ..HostInfo::default()
            },
            tool_deadline: TOOL_DEADLINE,
            event_deadline: EVENT_DEADLINE,
            host_api: API_VERSION.into(),
        }
    }

    /// As the user set the plugin up in config.toml.
    pub fn from_config(config: &PluginConfig, base: &Path) -> Settings {
        Settings { grants: config.grants.clone(), memory_mb: config.memory_mb, ..Settings::new(base) }
    }
}

pub struct Plugin {
    pre: MeshgrovePluginPre<State>,
    manifest: Manifest,
    tools: Vec<ToolDef>,
    hash: String,
    from_cache: bool,
    pub(crate) settings: Settings,
}

/// A picture a tool returned.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub mime: String,
    pub data: Vec<u8>,
}

/// What a tool returned.
#[derive(Debug)]
pub struct ToolOutcome {
    /// For the AI.
    pub result: Value,
    pub images: Vec<Image>,
    pub bundle: Option<StagedBundle>,
}

/// A bundle and the staging folder its files are in.
#[derive(Debug)]
pub struct StagedBundle {
    /// The plugin's id.
    pub plugin: String,
    pub bundle: Bundle,
    pub staging: Staging,
    /// The library files the bundle puts its own beside (`next_to`), found when it was handed
    /// over, as far as whoever handed it over may read the library.
    pub sources: Vec<Source>,
}

impl StagedBundle {
    /// A bundle that goes into a folder of its own.
    pub fn new(plugin: &str, bundle: Bundle, staging: Staging) -> StagedBundle {
        StagedBundle { plugin: plugin.to_owned(), bundle, staging, sources: Vec::new() }
    }

    /// Checks the bundle against its files and finds its place in library `root` (by default
    /// below `landing`), or beside the library files it names, wherever those are.
    pub fn resolve(&self, root: &Path, landing: &Path) -> Result<ResolvedBundle, Error> {
        let mut resolved = match self.bundle.uses_next_to() {
            true => meshgrove_core::bundle::resolve_beside(
                &self.bundle,
                self.staging.out(),
                &self.sources,
                Limits::DEFAULT,
            )?,
            false => meshgrove_core::bundle::resolve(&self.bundle, self.staging.out(), root, landing)?,
        };
        resolved.plugin = self.plugin.clone();
        Ok(resolved)
    }

    /// [`StagedBundle::resolve`] into the library imports go to.
    pub fn resolve_for(&self, config: &Config) -> Result<ResolvedBundle, Error> {
        match (config.import_root(), config.landing_dir()) {
            (Some(root), Some(landing)) => self.resolve(root, &landing),
            // Beside a library file: the library is the one that file is in.
            _ if self.bundle.uses_next_to() => self.resolve(Path::new(""), Path::new("")),
            _ => Err(Error::Invalid("there is no library to put the files into".into())),
        }
    }
}

fn load_error(e: wasmtime::Error) -> Error {
    Error::Load(e.to_string())
}

fn plugin_error(e: PluginError) -> Error {
    match e {
        PluginError::Invalid(m) => Error::Invalid(m),
        PluginError::Failed(m) => Error::Failed(m),
        PluginError::Denied(m) => Error::Denied(m),
        PluginError::Cancelled => Error::Cancelled,
    }
}

impl Plugin {
    pub fn load(file: &Path, settings: Settings) -> Result<Plugin, Error> {
        Plugin::from_bytes(&std::fs::read(file)?, settings)
    }

    /// Compiles (or takes from the cache), links and asks the plugin for its manifest and
    /// tools, which must be usable.
    pub fn from_bytes(bytes: &[u8], settings: Settings) -> Result<Plugin, Error> {
        let cache = settings.cache.then(|| settings.base.join(".cache"));
        let compiled = crate::engine::compile(bytes, cache.as_deref())?;
        // The world it links against has its own version, which no 0.x API changes; what the
        // plugin needs of the JSON shapes its manifest says (checked below).
        match crate::engine::exported_api(&compiled.component) {
            Some(v) => meshgrove_plugin_api::api_check(WIT_VERSION, &v).map_err(Error::Load)?,
            None => return Err(Error::Load("not a Meshgrove plugin: it exports no meshgrove:plugin/plugin".into())),
        }
        let mut linker = Linker::new(crate::engine::engine());
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(load_error)?;
        MeshgrovePlugin::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s).map_err(load_error)?;
        let pre = linker.instantiate_pre(&compiled.component).map_err(load_error)?;
        let pre = MeshgrovePluginPre::new(pre).map_err(load_error)?;
        let mut plugin = Plugin {
            pre,
            manifest: Manifest::default(),
            tools: Vec::new(),
            hash: compiled.hash,
            from_cache: compiled.from_cache,
            settings,
        };
        let (manifest, tools) = plugin.introduce()?;
        manifest.check_for(&plugin.settings.host_api).map_err(Error::Load)?;
        meshgrove_plugin_api::manifest::check_tools(&manifest.id, &tools).map_err(Error::Load)?;
        plugin.manifest = manifest;
        plugin.tools = tools;
        Ok(plugin)
    }

    /// Manifest and tools, from an instance with nothing granted and empty folders.
    fn introduce(&self) -> Result<(Manifest, Vec<ToolDef>), Error> {
        let dir = self.settings.base.join(".load").join(new_uid());
        let result = (|| {
            let mut store = state::store(Setup {
                reading: Reading::nothing("(loading)"),
                info: &self.settings.info,
                memory_mb: DEFAULT_MEMORY_MB,
                folders: Folders {
                    data: &dir.join("data"),
                    data_writable: false,
                    out: &dir.join("out"),
                    tmp_base: &dir,
                    tmp_exact: false,
                    scratch: &dir,
                },
                cancel: CancelToken::new(),
                progress: None,
            })?;
            state::arm(&mut store, Some(EVENT_DEADLINE));
            let instance = self.pre.instantiate(&mut store).map_err(|e| state::failure(&store, &e))?;
            let api = instance.meshgrove_plugin_plugin();
            let manifest = api.call_manifest(&mut store).map_err(|e| state::failure(&store, &e))?;
            let tools = api.call_list_tools(&mut store).map_err(|e| state::failure(&store, &e))?;
            // The API it was built for first: a manifest of a newer one may hold what this
            // Meshgrove cannot read, and then "update Meshgrove" is the answer, not a parse error.
            #[derive(serde::Deserialize)]
            struct Api {
                api: String,
            }
            if let Ok(Api { api }) = serde_json::from_str(&manifest) {
                meshgrove_plugin_api::api_check(&self.settings.host_api, &api).map_err(Error::Load)?;
            }
            let manifest: Manifest =
                serde_json::from_str(&manifest).map_err(|e| Error::Load(format!("its manifest: {e}")))?;
            let tools: Vec<ToolDef> =
                serde_json::from_str(&tools).map_err(|e| Error::Load(format!("its tool list: {e}")))?;
            Ok((manifest, tools))
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn tools(&self) -> &[ToolDef] {
        &self.tools
    }

    /// blake3 of the `.wasm` file.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// The compiled code came from the cache.
    pub fn from_cache(&self) -> bool {
        self.from_cache
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Memory of one instance in MiB.
    pub fn memory_mb(&self) -> u32 {
        self.settings.memory_mb.or(self.manifest.memory_mb).unwrap_or(DEFAULT_MEMORY_MB).clamp(16, 4096)
    }

    /// Whether the plugin asks for `permission` and the user granted it.
    pub fn may(&self, permission: Permission) -> bool {
        let g = &self.settings.grants;
        self.manifest.wants(permission)
            && match permission {
                Permission::AiTools => g.ai_tools,
                Permission::Bundles => g.bundles,
                Permission::LibraryRead => g.library_read != LibraryRead::None,
                Permission::Unknown => false,
            }
    }

    fn require(&self, permission: Permission) -> Result<(), Error> {
        if self.may(permission) {
            return Ok(());
        }
        let what = match permission {
            Permission::AiTools => "offer tools to the AI",
            Permission::Bundles => "put files into the library",
            Permission::LibraryRead => "read the library",
            Permission::Unknown => "do this",
        };
        Err(Error::Denied(format!("the user has not allowed {} to {what}", self.manifest.id)))
    }

    /// Whether the user's entry in config.toml lets this very file run.
    pub fn check_config(&self, config: &PluginConfig) -> Result<(), Error> {
        if config.id != self.manifest.id {
            return Err(Error::Load(format!("the file is plugin {:?}, not {:?}", self.manifest.id, config.id)));
        }
        if !config.usable(&self.hash) {
            return Err(Error::Denied(format!("{} is switched off or changed since it was enabled", config.id)));
        }
        Ok(())
    }

    /// The plugin's own folder (`/data`).
    pub fn data_dir(&self) -> PathBuf {
        match &self.settings.data {
            Some(data) => data.clone(),
            None => self.settings.base.join(&self.manifest.id).join("data"),
        }
    }

    /// The plugin's temporary folders: a `/tmp` for each instance that has none of its own, and
    /// what the host puts on disk for it.
    fn scratch_dir(&self) -> PathBuf {
        self.settings.base.join(&self.manifest.id).join("tmp")
    }

    /// How an instance reads the library for `caller`: with the plugin's grant, and the files
    /// in `opened`.
    pub(crate) fn reading(&self, caller: Caller, opened: Opened) -> Reading {
        Reading {
            plugin: self.manifest.id.clone(),
            library: self.settings.library.clone(),
            grant: self.may(Permission::LibraryRead).then_some(self.settings.grants.library_read),
            caller,
            opened,
        }
    }

    /// A store for one instance, with `out` as `/out` and a fresh `/tmp` (or `tmp`), reading
    /// the library as `reading` says.
    pub(crate) fn store(
        &self,
        out: &Path,
        tmp: Option<&Path>,
        data_writable: bool,
        reading: Reading,
        cancel: CancelToken,
        progress: Option<Progress>,
    ) -> Result<Store<State>, Error> {
        let (data, scratch) = (self.data_dir(), self.scratch_dir());
        let tmp_exact = tmp.is_some();
        let tmp_base = tmp.map_or_else(|| scratch.clone(), Path::to_path_buf);
        state::store(Setup {
            reading,
            info: &self.settings.info,
            memory_mb: self.memory_mb(),
            folders: Folders { data: &data, data_writable, out, tmp_base: &tmp_base, tmp_exact, scratch: &scratch },
            cancel,
            progress,
        })
    }

    /// A bundle with the staging folder its files are in, ready to be resolved. The files it
    /// asks the host to make are made now (a 3MF with parts swapped), and the library files it
    /// puts its own beside are found, both as far as `reading` reaches; `tmp` is the `/tmp`
    /// of whoever hands it over, if that is still there. When any of it fails the staging
    /// folder goes.
    pub(crate) fn stage(
        &self,
        bundle: Bundle,
        staging: Staging,
        reading: &Reading,
        tmp: Option<&Path>,
    ) -> Result<StagedBundle, Error> {
        let sources = (|| {
            let dirs =
                GuestDirs { data: self.data_dir(), tmp: tmp.map(Path::to_path_buf), scratch: self.scratch_dir() };
            crate::derive::derive_files(&bundle, staging.out(), reading, &dirs)?;
            let mut sources = Vec::new();
            for reference in bundle.sources() {
                // Beside a part is beside the file it is in.
                let (id, _) = crate::library::split_part(reference).map_err(Error::Invalid)?;
                if crate::guest_files::is_guest_path(id) {
                    return Err(Error::Invalid(format!("next_to takes the id of a library file, not a path ({id})")));
                }
                let found = reading.locate(id).map_err(Error::Denied)?;
                sources.push(Source { reference: reference.to_owned(), root: found.root, path: found.path });
            }
            Ok(sources)
        })();
        match sources {
            Ok(sources) => Ok(StagedBundle { plugin: self.manifest.id.clone(), bundle, staging, sources }),
            Err(e) => {
                let _ = staging.remove();
                Err(e)
            }
        }
    }

    pub(crate) fn instantiate(&self, store: &mut Store<State>) -> Result<MeshgrovePlugin, Error> {
        self.pre.instantiate(&mut *store).map_err(|e| state::failure(store, &e))
    }

    /// Calls tool `name` (as the plugin names it, without the `<id>_` prefix) for the user; the
    /// files it writes land in a new staging folder, kept when it returns a bundle.
    pub fn call_tool(&self, name: &str, args: &Value, cancel: &CancelToken) -> Result<ToolOutcome, Error> {
        self.call_tool_as(name, args, AccessClass::Local, cancel)
    }

    /// [`Plugin::call_tool`] for an AI of access class `class`: the library reads as it would.
    pub fn call_tool_as(
        &self,
        name: &str,
        args: &Value,
        class: AccessClass,
        cancel: &CancelToken,
    ) -> Result<ToolOutcome, Error> {
        self.call_tool_in(name, args, Staging::new(&self.settings.base, &self.manifest.id)?, class, cancel)
    }

    /// [`Plugin::call_tool_as`] writing into `staging`.
    pub fn call_tool_in(
        &self,
        name: &str,
        args: &Value,
        staging: Staging,
        class: AccessClass,
        cancel: &CancelToken,
    ) -> Result<ToolOutcome, Error> {
        match self.run_tool(name, args, staging.out(), class, cancel) {
            // Files a job of the plugin is to make: they are staged once it has made them
            // ([`Plugin::finish_job`]); the staging folder waits for it.
            Ok((result, images, Some(bundle))) if bundle.job.is_some() => {
                Ok(ToolOutcome { result, images, bundle: Some(StagedBundle::new(&self.manifest.id, bundle, staging)) })
            }
            Ok((result, images, Some(bundle))) => {
                // What a tool puts its files beside, the AI that called it must be able to read.
                let reading = self.reading(Caller::Ai(class), Opened::default());
                // Its own /tmp went with its instance.
                Ok(ToolOutcome { result, images, bundle: Some(self.stage(bundle, staging, &reading, None)?) })
            }
            Ok((result, images, None)) => {
                let _ = staging.remove();
                Ok(ToolOutcome { result, images, bundle: None })
            }
            Err(e) => {
                let _ = staging.remove();
                Err(e)
            }
        }
    }

    fn run_tool(
        &self,
        name: &str,
        args: &Value,
        out: &Path,
        class: AccessClass,
        cancel: &CancelToken,
    ) -> Result<(Value, Vec<Image>, Option<Bundle>), Error> {
        self.require(Permission::AiTools)?;
        if !self.tools.iter().any(|t| t.name == name) {
            return Err(Error::Invalid(format!("{} has no tool {name:?}", self.manifest.id)));
        }
        let reading = self.reading(Caller::Ai(class), Opened::default());
        let mut store = self.store(out, None, false, reading, cancel.clone(), None)?;
        state::arm(&mut store, Some(self.settings.tool_deadline));
        let instance = self.instantiate(&mut store)?;
        let output = instance
            .meshgrove_plugin_plugin()
            .call_call_tool(&mut store, name, &args.to_string())
            .map_err(|e| state::failure(&store, &e))?
            .map_err(plugin_error)?;
        if output.result.len() > MAX_RESULT {
            return Err(Error::Failed("its answer is larger than 1 MiB".into()));
        }
        let images_ok = output.images.len() <= MAX_IMAGES
            && output
                .images
                .iter()
                .all(|i| i.data.len() <= MAX_IMAGE && matches!(i.mime.as_str(), "image/png" | "image/jpeg"));
        if !images_ok {
            return Err(Error::Failed(format!("pictures: at most {MAX_IMAGES} PNG or JPEG of 8 MiB each")));
        }
        let result = serde_json::from_str(&output.result).unwrap_or(Value::String(output.result));
        let images = output.images.into_iter().map(|i| Image { mime: i.mime, data: i.data }).collect();
        let bundle = match output.bundle {
            None => None,
            Some(text) => {
                self.require(Permission::Bundles)?;
                Some(serde_json::from_str(&text).map_err(|e| Error::Failed(format!("its bundle: {e}")))?)
            }
        };
        Ok((result, images, bundle))
    }

    /// Runs job `kind` until it is done or cancelled, writing into `out` (a panel's `/out`).
    pub fn run_job(
        &self,
        kind: &str,
        params: &Value,
        out: &Path,
        cancel: &CancelToken,
        progress: Option<Progress>,
    ) -> Result<Value, Error> {
        self.run_job_with(kind, params, out, &Opened::default(), cancel, progress)
    }

    /// [`Plugin::run_job`] for a panel: the job reads the files in `opened` as the panel does.
    pub fn run_job_with(
        &self,
        kind: &str,
        params: &Value,
        out: &Path,
        opened: &Opened,
        cancel: &CancelToken,
        progress: Option<Progress>,
    ) -> Result<Value, Error> {
        let reading = self.reading(Caller::User, opened.clone());
        self.run_job_as(kind, params, out, reading, true, cancel, progress)
    }

    /// Runs job `kind` reading the library as `reading` says, with `/data` writable or not.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_job_as(
        &self,
        kind: &str,
        params: &Value,
        out: &Path,
        reading: Reading,
        data_writable: bool,
        cancel: &CancelToken,
        progress: Option<Progress>,
    ) -> Result<Value, Error> {
        let mut store = self.store(out, None, data_writable, reading, cancel.clone(), progress)?;
        state::arm(&mut store, None);
        let instance = self.instantiate(&mut store)?;
        let text = instance
            .meshgrove_plugin_plugin()
            .call_run_job(&mut store, kind, &params.to_string())
            .map_err(|e| state::failure(&store, &e))?
            .map_err(plugin_error)?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    /// Runs the job a tool's bundle asks for ([`Bundle::job`]) and stages what it made: the
    /// bundle in its answer's `bundle` member if it has one (else the tool's, without the job),
    /// with the files it wrote to the bundle's staging folder. The job runs as the tool did: it
    /// reads the library as the AI of access class `class` that called the tool, `/data` is
    /// read-only, and there is no deadline; `cancel` stops it. Whatever goes wrong, the staging
    /// folder goes too.
    pub fn finish_job(
        &self,
        staged: StagedBundle,
        class: AccessClass,
        cancel: &CancelToken,
        progress: Option<Progress>,
    ) -> Result<StagedBundle, Error> {
        let StagedBundle { bundle, staging, .. } = staged;
        let made = (|| {
            let job = bundle.job.clone().ok_or_else(|| Error::Invalid("the bundle starts no job".into()))?;
            let reading = self.reading(Caller::Ai(class), Opened::default());
            let answer = self.run_job_as(&job.kind, &job.params, staging.out(), reading, false, cancel, progress)?;
            let bundle = match answer.get("bundle") {
                Some(given) => serde_json::from_value::<Bundle>(given.clone())
                    .map_err(|e| Error::Failed(format!("the bundle of its job's answer: {e}")))?,
                None => Bundle { job: None, ..bundle },
            };
            if bundle.job.is_some() {
                return Err(Error::Invalid("a job's bundle cannot start another job".into()));
            }
            Ok(bundle)
        })();
        match made {
            // What it puts its files beside, the AI that called the tool must be able to read.
            Ok(bundle) => self.stage(bundle, staging, &self.reading(Caller::Ai(class), Opened::default()), None),
            Err(e) => {
                let _ = staging.remove();
                Err(e)
            }
        }
    }

    /// Opens file `name` (one of the manifest's file types) with the plugin: what it asks
    /// Meshgrove to do (usually open a panel).
    pub fn open_file(&self, name: &str, bytes: &[u8]) -> Result<Vec<Effect>, Error> {
        if !self.manifest.file_types.iter().any(|t| t.matches(name)) {
            return Err(Error::Invalid(format!("{} does not open {name:?}", self.manifest.id)));
        }
        let staging = Staging::new(&self.settings.base, &self.manifest.id)?;
        let result = (|| {
            let reading = self.reading(Caller::User, Opened::default());
            let mut store = self.store(staging.out(), None, false, reading, CancelToken::new(), None)?;
            state::arm(&mut store, Some(self.settings.tool_deadline));
            let instance = self.instantiate(&mut store)?;
            let text = instance
                .meshgrove_plugin_plugin()
                .call_open_file(&mut store, name, bytes)
                .map_err(|e| state::failure(&store, &e))?
                .map_err(plugin_error)?;
            effects(&self.manifest.id, &text)
        })();
        let _ = staging.remove();
        result
    }
}

/// The effects a plugin answered with (JSON). Those this Meshgrove does not know (from a newer
/// API) are logged and left out; the others are done.
pub(crate) fn effects(plugin: &str, text: &str) -> Result<Vec<Effect>, Error> {
    let mut effects: Vec<Effect> =
        serde_json::from_str(text).map_err(|e| Error::Failed(format!("its effects: {e}")))?;
    let before = effects.len();
    effects.retain(|e| *e != Effect::Unknown);
    if effects.len() < before {
        log::warn!(
            target: "plugin",
            "{plugin}: skipped {} effect(s) this Meshgrove does not know (the plugin is built for a newer one)",
            before - effects.len()
        );
    }
    Ok(effects)
}
