//! The plugins the user enabled (`[[plugins]]` in config.toml), loaded, and their tools for the
//! AI: named `<plugin>_<tool>`, offered to the chat through [`ToolProvider`] and served by
//! `meshgrove-mcp`. A bundle a tool returns becomes a suggestion like any other.

use std::path::Path;
use std::sync::Arc;

use meshgrove_core::ai::dispatch::{self, Cancel, Picture, Reply, ToolProvider};
use meshgrove_core::ai::proposals::{self, Origin, ProposalStore};
use meshgrove_core::ai::{Context, ToolError, ToolResult};
use meshgrove_core::config::Config;
use meshgrove_plugin_api::{API_VERSION, HostInfo, Permission, PrinterInfo, ToolDef};
use serde_json::{Value, json};

use crate::deferred::{DeferredJobs, Deliver};
use crate::{CancelToken, Image, LibraryAccess, Plugin, Settings};

/// The loaded plugins.
#[derive(Default)]
pub struct Registry {
    plugins: Vec<Arc<Plugin>>,
    /// Enabled plugins that could not be loaded: id and why.
    problems: Vec<(String, String)>,
    /// The jobs their tools started; shared with the registries loaded before and after.
    jobs: Arc<DeferredJobs>,
}

/// What the host tells plugins about itself, from the settings.
pub fn host_info(config: &Config, language: &str, dark: bool) -> HostInfo {
    HostInfo {
        language: language.to_owned(),
        dark,
        printers: config
            .printers_in_order()
            .into_iter()
            .map(|p| PrinterInfo { name: p.name, bed: [p.x, p.y, p.z], nozzle: p.nozzle })
            .collect(),
        meshgrove: env!("CARGO_PKG_VERSION").into(),
        api: API_VERSION.into(),
        imperial: config.display_units == meshgrove_core::config::DisplayUnits::Imperial,
    }
}

impl Registry {
    /// Loads the enabled plugins of `config` whose file is still the one the user enabled;
    /// installed below `base` ([`crate::default_base`]).
    pub fn load(config: &Config, base: &Path, info: &HostInfo, library: Option<Arc<dyn LibraryAccess>>) -> Registry {
        Registry::load_with_jobs(config, base, info, library, DeferredJobs::new())
    }

    /// [`Registry::load`] keeping the tool-started `jobs` of the process (they go on while the
    /// plugins load anew).
    pub fn load_with_jobs(
        config: &Config,
        base: &Path,
        info: &HostInfo,
        library: Option<Arc<dyn LibraryAccess>>,
        jobs: Arc<DeferredJobs>,
    ) -> Registry {
        let mut registry = Registry { jobs, ..Registry::default() };
        for entry in config.plugins.iter().filter(|p| p.enabled) {
            let settings =
                Settings { library: library.clone(), info: info.clone(), ..Settings::from_config(entry, base) };
            let loaded = Plugin::load(&entry.file, settings).and_then(|p| p.check_config(entry).map(|()| p));
            let problem = match loaded {
                Err(e) => Some(e.to_string()),
                Ok(_) if registry.get(&entry.id).is_some() => Some("installed twice".to_owned()),
                Ok(p) => match p.tools().iter().map(|t| full_name(p.id(), &t.name)).find(|n| dispatch::is_fixed(n)) {
                    Some(name) => Some(format!("its tool {name} is named like one of Meshgrove's")),
                    None => {
                        registry.plugins.push(Arc::new(p));
                        None
                    }
                },
            };
            if let Some(why) = problem {
                log::warn!("plugin {}: {why}", entry.id);
                registry.problems.push((entry.id.clone(), why));
            }
        }
        registry
    }

    /// The jobs the plugins' tools started.
    pub fn jobs(&self) -> &Arc<DeferredJobs> {
        &self.jobs
    }

    pub fn plugins(&self) -> &[Arc<Plugin>] {
        &self.plugins
    }

    pub fn get(&self, id: &str) -> Option<&Arc<Plugin>> {
        self.plugins.iter().find(|p| p.id() == id)
    }

    /// Enabled plugins that could not be loaded: id and why.
    pub fn problems(&self) -> &[(String, String)] {
        &self.problems
    }

    /// The tools offered to the AI: full name, plugin and tool.
    pub fn ai_tools(&self) -> impl Iterator<Item = (String, &Arc<Plugin>, &ToolDef)> {
        self.plugins
            .iter()
            .filter(|p| p.may(Permission::AiTools))
            .flat_map(|p| p.tools().iter().map(move |t| (full_name(p.id(), &t.name), p, t)))
    }

    /// The plugin and tool behind `<plugin>_<tool>`, if offered to the AI.
    pub fn tool(&self, name: &str) -> Option<(&Arc<Plugin>, &ToolDef)> {
        // Plugin ids have no '_', so the first one ends the id.
        let (id, tool) = name.split_once('_')?;
        let plugin = self.get(id).filter(|p| p.may(Permission::AiTools))?;
        Some((plugin, plugin.tools().iter().find(|t| t.name == tool)?))
    }

    /// Calls tool `name` for the AI (stopped by `cancel`). Its files become a suggestion in `store` from `origin`;
    /// the answer says which, beside the plugin's own (data, not instructions).
    pub fn call_ai(
        &self,
        cx: &Context,
        store: &mut ProposalStore,
        origin: &Origin,
        name: &str,
        arguments: Value,
        cancel: &CancelToken,
    ) -> ToolResult<(Value, Vec<Image>)> {
        let (plugin, tool) = self.tool(name).ok_or_else(|| ToolError::Invalid(format!("no tool called {name}")))?;
        let arguments = match arguments {
            Value::Null => json!({}),
            Value::String(s) => serde_json::from_str(&s).unwrap_or_else(|_| json!({})),
            v => v,
        };
        let outcome = plugin.call_tool_as(&tool.name, &arguments, origin.class, cancel).map_err(invalid)?;
        let mut answer = json!({});
        let mut bundle = outcome.bundle;
        if let Some(staged) = bundle.take_if(|s| s.bundle.job.is_some()) {
            // The files come from a job: it runs now, and they wait in the review when it is done.
            let job = self.start_job(cx, store, origin, name, tool, plugin, staged)?;
            answer = json!({"status": "started", "job": job, "arrives": "review"});
        }
        if let Some(staged) = bundle {
            let receipt = (|| {
                if !tool.bundles {
                    return Err(ToolError::Invalid(format!("{name} returned files, but says it makes none")));
                }
                let resolved = staged.resolve_for(cx.config).map_err(invalid)?;
                let receipt = proposals::propose_bundle(cx.config, store, origin, &resolved)?;
                Ok((receipt, cx.id_of(&resolved.to)))
            })();
            match receipt {
                Ok((receipt, place)) => {
                    answer = serde_json::to_value(receipt).map_err(|e| ToolError::Invalid(e.to_string()))?;
                    if !place.is_empty() {
                        answer["folder"] = json!(place);
                    }
                }
                Err(e) => {
                    let _ = staged.staging.remove();
                    return Err(e);
                }
            }
        }
        answer["plugin"] = json!(plugin.id());
        answer["result"] = outcome.result;
        if !outcome.images.is_empty() {
            answer["pictures"] = json!(outcome.images.len());
        }
        Ok((answer, outcome.images))
    }
}

impl Registry {
    /// Starts the job of a tool's bundle; its id. What it makes is proposed into the store
    /// `store` is (its file, opened anew then) from `origin`, and always waits in the review.
    #[allow(clippy::too_many_arguments)]
    fn start_job(
        &self,
        cx: &Context,
        store: &ProposalStore,
        origin: &Origin,
        name: &str,
        tool: &ToolDef,
        plugin: &Arc<Plugin>,
        staged: crate::StagedBundle,
    ) -> ToolResult<String> {
        let refuse = |why: String, staged: crate::StagedBundle| {
            let _ = staged.staging.remove();
            Err(ToolError::Invalid(why))
        };
        if !tool.bundles {
            return refuse(format!("{name} returned files, but says it makes none"), staged);
        }
        let Some(path) = store.path() else {
            return refuse("there is no suggestion store to put the job's files into".into(), staged);
        };
        let (config, from) = (cx.config.clone(), origin.clone());
        let deliver: Deliver = Box::new(move |staged: crate::StagedBundle| {
            let proposed = (|| {
                let mut resolved = staged.resolve_for(&config).map_err(|e| e.to_string())?;
                resolved.review = true;
                let mut store = ProposalStore::open(&path).map_err(|e| e.to_string())?;
                proposals::propose_bundle(&config, &mut store, &from, &resolved).map_err(|e| e.to_string())?;
                Ok(resolved.title)
            })();
            if proposed.is_err() {
                let _ = staged.staging.remove();
            }
            proposed
        });
        Ok(self.jobs.start(plugin.clone(), &tool.name, staged, origin.class, deliver))
    }
}

/// `<plugin>_<tool>`
pub fn full_name(plugin: &str, tool: &str) -> String {
    format!("{plugin}_{tool}")
}

fn invalid(e: crate::Error) -> ToolError {
    ToolError::Invalid(e.to_string())
}

impl ToolProvider for Registry {
    fn definitions(&self) -> Vec<Value> {
        self.ai_tools()
            .map(|(name, _, t)| {
                json!({"type": "function", "function": {"name": name, "description": t.description, "parameters": t.input_schema}})
            })
            .collect()
    }

    fn writes(&self, name: &str) -> Option<bool> {
        self.tool(name).map(|(_, t)| t.bundles)
    }

    fn call(
        &self,
        cx: &Context,
        store: &mut ProposalStore,
        origin: &Origin,
        name: &str,
        arguments: Value,
        cancel: &Cancel,
    ) -> ToolResult<Reply> {
        let cancel = CancelToken::from_flag(cancel.clone());
        let (value, images) = self.call_ai(cx, store, origin, name, arguments, &cancel)?;
        let pictures = images.into_iter().map(|i| Picture { mime: i.mime, data: i.data }).collect();
        Ok(Reply { value, pictures })
    }

    fn needs_review(&self, name: &str) -> bool {
        self.tool(name).is_some_and(|(p, _)| !p.settings().grants.auto_accept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshgrove_core::config::DisplayUnits;
    use meshgrove_core::printer::Printer;

    #[test]
    fn the_host_tells_units_and_nozzles() {
        let printer = |name: &str, nozzle| Printer {
            name: name.into(),
            x: 250.0,
            y: 250.0,
            z: 260.0,
            nozzle,
            materials: Vec::new(),
            gcode_names: Vec::new(),
        };
        let printers = Some(vec![printer("Kobra", Some(0.6)), printer("Old", None)]);
        let mut config = Config { printers, ..Config::default() };
        let info = host_info(&config, "de", true);
        assert!(!info.imperial && info.dark);
        assert_eq!(info.api, API_VERSION);
        let nozzles: Vec<Option<f32>> = info.printers.iter().map(|p| p.nozzle).collect();
        assert!(nozzles.contains(&Some(0.6)) && nozzles.contains(&None), "{nozzles:?}");
        config.display_units = DisplayUnits::Imperial;
        assert!(host_info(&config, "de", true).imperial);
    }
}
