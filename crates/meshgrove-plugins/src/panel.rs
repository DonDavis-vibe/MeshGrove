//! An open panel: one instance of the plugin, kept as long as its tab, on a thread of its own
//! (never the UI thread). Requests go to it over a channel; each has the event deadline
//! ([`crate::EVENT_DEADLINE`] unless the settings say otherwise).
//! After a trap (a crash, a deadline) the instance cannot be entered again and every request
//! says so; the tab has to open the panel anew.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use meshgrove_plugin_api::MeshData;
use meshgrove_plugin_api::ui::{Effect, Event, View};
use wasmtime::Store;
use wasmtime::component::ResourceAny;

use crate::Error;
use crate::bindings::MeshgrovePlugin;
use crate::plugin::Plugin;
use crate::staging::{Staging, new_uid};
use crate::state::{self, CancelToken, State};

type Reply<T> = Sender<Result<T, Error>>;

enum Request {
    /// What the host tells the plugin about itself changed (before `Event::HostChanged`).
    Info(meshgrove_plugin_api::HostInfo, Reply<()>),
    Render(Reply<View>),
    Event(String, Reply<Vec<Effect>>),
    Mesh(String, Reply<Option<MeshData>>),
    Snapshot(Reply<String>),
}

pub struct PanelHandle {
    plugin: Arc<Plugin>,
    panel: String,
    requests: Option<Sender<Request>>,
    thread: Option<JoinHandle<()>>,
    /// The panel's `/out`, shared with the jobs it starts.
    out: PathBuf,
    /// The panel's `/tmp`.
    tmp: PathBuf,
}

impl Plugin {
    /// Opens panel `panel` of the manifest, restoring `state` (an earlier snapshot).
    pub fn open_panel(self: &Arc<Self>, panel: &str, state: Option<String>) -> Result<PanelHandle, Error> {
        if !self.manifest().panels.iter().any(|p| p.id == panel) {
            return Err(Error::Invalid(format!("{} has no panel {panel:?}", self.id())));
        }
        let dir = self.settings.base.join(self.id()).join("panels").join(new_uid());
        let (out, tmp) = (dir.join("out"), dir.join("tmp"));
        std::fs::create_dir_all(&out)?;
        let (requests, queue) = crossbeam_channel::unbounded();
        let (ready, opened) = crossbeam_channel::bounded(1);
        let thread = {
            let (plugin, panel, out, tmp) = (self.clone(), panel.to_owned(), out.clone(), tmp.clone());
            std::thread::Builder::new()
                .name(format!("plugin-panel-{}", self.id()))
                .spawn(move || run(&plugin, &panel, state, (&out, &tmp), &ready, &queue))?
        };
        let handle = PanelHandle {
            plugin: self.clone(),
            panel: panel.to_owned(),
            requests: Some(requests),
            thread: Some(thread),
            out,
            tmp,
        };
        match opened.recv() {
            Ok(Ok(())) => Ok(handle),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::Trap("the panel's thread ended".into())),
        }
    }
}

impl PanelHandle {
    pub fn plugin(&self) -> &Arc<Plugin> {
        &self.plugin
    }

    /// The panel's id in the manifest.
    pub fn id(&self) -> &str {
        &self.panel
    }

    pub fn render(&self) -> Result<View, Error> {
        self.ask(Request::Render)
    }

    /// The effects the plugin asks for in answer to `event`.
    pub fn event(&self, event: &Event) -> Result<Vec<Effect>, Error> {
        // From now on `host.info()` answers the new facts too.
        if let Event::HostChanged { info } = event {
            self.ask(|reply| Request::Info(info.clone(), reply))?;
        }
        let json = serde_json::to_string(event).map_err(|e| Error::Invalid(e.to_string()))?;
        self.ask(|reply| Request::Event(json, reply))
    }

    /// The triangles of the mesh view with `key`.
    pub fn mesh(&self, key: &str) -> Result<Option<MeshData>, Error> {
        self.ask(|reply| Request::Mesh(key.to_owned(), reply))
    }

    /// The panel's state, to open it again later as it is.
    pub fn snapshot(&self) -> Result<String, Error> {
        self.ask(Request::Snapshot)
    }

    /// The panel's `/out`, where it and its jobs write.
    pub fn out(&self) -> &Path {
        &self.out
    }

    /// The panel's `/tmp` (picked files go there).
    pub fn tmp(&self) -> &Path {
        &self.tmp
    }

    /// What the panel and its jobs wrote so far, moved into a staging folder of its own (for
    /// a bundle); the panel's `/out` is empty again.
    pub fn take_output(&self) -> Result<Staging, Error> {
        Ok(Staging::take_from(&self.out, &self.plugin.settings.base, self.plugin.id())?)
    }

    fn ask<T>(&self, request: impl FnOnce(Reply<T>) -> Request) -> Result<T, Error> {
        let gone = || Error::Trap("the panel is closed".into());
        let (reply, answer) = crossbeam_channel::bounded(1);
        self.requests.as_ref().ok_or_else(gone)?.send(request(reply)).map_err(|_| gone())?;
        answer.recv().map_err(|_| gone())?
    }
}

impl Drop for PanelHandle {
    fn drop(&mut self) {
        // Closing the queue ends the thread, which drops the panel in the plugin.
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(dir) = self.out.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// The panel's thread: opens the panel, then answers requests until the handle is dropped.
fn run(
    plugin: &Plugin,
    panel: &str,
    snapshot: Option<String>,
    (out, tmp): (&Path, &Path),
    ready: &Sender<Result<(), Error>>,
    queue: &Receiver<Request>,
) {
    let opened = (|| {
        // A panel reads the library as the user, locally.
        let mut store =
            plugin.store(out, Some(tmp), true, meshgrove_core::config::AccessClass::Local, CancelToken::new(), None)?;
        state::arm(&mut store, Some(plugin.settings.event_deadline));
        let instance = plugin.instantiate(&mut store)?;
        let resource = instance
            .meshgrove_plugin_plugin()
            .panel()
            .call_constructor(&mut store, panel, snapshot.as_deref())
            .map_err(|e| state::failure(&store, &e))?;
        Ok((store, instance, resource))
    })();
    let (mut store, instance, resource) = match opened {
        Ok(opened) => {
            let _ = ready.send(Ok(()));
            opened
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let deadline = plugin.settings.event_deadline;
    let mut open = Open { store: &mut store, instance: &instance, resource, deadline, dead: None };
    for request in queue {
        match request {
            Request::Info(info, reply) => {
                open.store.data_mut().info = info;
                let _ = reply.send(Ok(()));
            }
            Request::Render(reply) => {
                let view = open
                    .call(|s, p, r| p.call_render(s, r))
                    .and_then(|text| serde_json::from_str(&text).map_err(|e| Error::Failed(format!("its view: {e}"))));
                let _ = reply.send(view);
            }
            Request::Event(event, reply) => {
                let effects = open
                    .call(|s, p, r| p.call_event(s, r, &event))
                    .and_then(|text| crate::plugin::effects(plugin.id(), &text));
                let _ = reply.send(effects);
            }
            Request::Mesh(key, reply) => {
                let mesh = open.call(|s, p, r| p.call_mesh(s, r, &key)).and_then(|mesh| {
                    let mesh = mesh.map(|m| MeshData { positions: m.positions, indices: m.indices });
                    match &mesh {
                        Some(m) => {
                            m.check().map(|()| mesh.clone()).map_err(|e| Error::Failed(format!("its mesh: {e}")))
                        }
                        None => Ok(None),
                    }
                });
                let _ = reply.send(mesh);
            }
            Request::Snapshot(reply) => {
                let _ = reply.send(open.call(|s, p, r| p.call_snapshot(s, r)));
            }
        }
    }
    if open.dead.is_none() {
        state::arm(&mut store, Some(deadline));
        let _ = resource.resource_drop(&mut store);
    }
}

/// A panel that is open in its instance.
struct Open<'a> {
    store: &'a mut Store<State>,
    instance: &'a MeshgrovePlugin,
    resource: ResourceAny,
    deadline: std::time::Duration,
    /// Why the instance cannot be entered again.
    dead: Option<String>,
}

impl Open<'_> {
    fn call<T>(
        &mut self,
        f: impl FnOnce(&mut Store<State>, crate::bindings::GuestPanel<'_>, ResourceAny) -> wasmtime::Result<T>,
    ) -> Result<T, Error> {
        if let Some(why) = &self.dead {
            return Err(Error::Trap(format!("the panel stopped earlier ({why}); open it again")));
        }
        state::arm(self.store, Some(self.deadline));
        f(self.store, self.instance.meshgrove_plugin_plugin().panel(), self.resource).map_err(|e| {
            let error = state::failure(self.store, &e);
            self.dead = Some(error.to_string());
            error
        })
    }
}
