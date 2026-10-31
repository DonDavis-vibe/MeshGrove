//! The tools for a chat model: their definitions in the OpenAI "tools" format, and calling one
//! by name with the model's JSON arguments. The same functions `meshgrove-mcp` serves.
//!
//! Besides the fixed tools, a [`ToolProvider`] may bring more: the tools of the user's plugins,
//! named `<plugin>_<tool>` ([`definitions_with`], [`writes_with`], [`call_with`]).

use serde::Deserialize;
use serde_json::{Value, json};

use super::proposals::{self, Origin, ProposalStore};
use super::types::ModelFilter;
use super::{Context, ToolError, ToolResult, read};

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IdArgs {
    /// A file or folder id.
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IdsArgs {
    /// Up to 100 file ids.
    pub ids: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InboxArgs {
    /// Only drops below this folder id.
    #[serde(default)]
    pub folder: Option<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListArgs {
    /// The list's name ("favorites" for the favourites).
    pub name: String,
}

#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DuplicatesArgs {
    #[serde(default)]
    pub library: Option<String>,
}

/// (name, description, writes)
const TOOLS: &[(&str, &str, bool)] = &[
    (
        "library_overview",
        "The libraries you may see, counts (inbox, untagged, problems), printers, most used tags.",
        false,
    ),
    (
        "search_models",
        "Find files. All given filters must match; q takes Meshgrove's search syntax (tag:, liste:, passt:, is:neu, ext:).",
        false,
    ),
    (
        "get_model",
        "Everything about one file: facts, 3MF metadata, note, lists, copies, printability, fit per printer, the folder's readme.",
        false,
    ),
    (
        "folder_summary",
        "A folder or archive: files by kind, subfolders, common tags, how many are new, readme, a sample.",
        false,
    ),
    ("list_inbox", "New files waiting in the inbox, grouped by the folder or archive they came in.", false),
    (
        "list_tags",
        "Tags in use (with counts and groups) and Meshgrove's suggested tags. Call before suggesting tags.",
        false,
    ),
    ("list_lists", "The user's lists: favourites, print lists, collections.", false),
    ("get_list", "The files of one list with copies wanted and done.", false),
    ("find_duplicates", "Groups of duplicate files.", false),
    ("list_printers", "The user's printers and build volumes; the first is the main one.", false),
    ("check_printability", "Printability of up to 100 files: mesh problems, suspicious sizes, fit per printer.", false),
    ("propose_tags", "Tags to add (and remove) per file.", true),
    ("propose_note", "A note per file (replacing or appended).", true),
    (
        "propose_list",
        "A new print list (with copies) or collection, or an automatic collection from a saved search.",
        true,
    ),
    (
        "propose_tag_merge",
        "Replace tags by one (spelling variants, synonyms, the other language) on every file carrying them.",
        true,
    ),
    (
        "propose_moves",
        "Move or rename files, archives or folders within their library; always waits for the user's review, never replaces anything.",
        true,
    ),
];

/// Whether the tool changes something (or suggests a change).
pub fn writes(name: &str) -> bool {
    TOOLS.iter().any(|(n, _, w)| *n == name && *w)
}

/// Whether `name` is one of the fixed tools (a provider's tool of that name is not offered).
pub fn is_fixed(name: &str) -> bool {
    TOOLS.iter().any(|(n, _, _)| *n == name)
}

/// More tools than the fixed ones: those of the user's plugins, named `<plugin>_<tool>`.
pub trait ToolProvider: Send + Sync {
    /// Its tools in the OpenAI "tools" format.
    fn definitions(&self) -> Vec<Value>;
    /// `None`: it has no tool `name`; else whether the tool changes something (or suggests a
    /// change).
    fn writes(&self, name: &str) -> Option<bool>;
    /// Calls its tool `name`; suggestions go to `store` as coming from `origin`.
    fn call(
        &self,
        cx: &Context,
        store: &mut ProposalStore,
        origin: &Origin,
        name: &str,
        arguments: Value,
    ) -> ToolResult<Value>;
    /// Whether what tool `name` suggests always waits for the review, also where suggestions
    /// are otherwise applied at once (the user did not let this plugin's be accepted by
    /// themselves).
    fn needs_review(&self, _name: &str) -> bool {
        false
    }
}

#[cfg(feature = "schema")]
fn tool_name(definition: &Value) -> &str {
    definition["function"]["name"].as_str().unwrap_or_default()
}

/// The fixed tools and those of `provider`; a provider's tool named like a fixed one (or like
/// one before it) is left out.
#[cfg(feature = "schema")]
pub fn definitions_with(provider: Option<&dyn ToolProvider>) -> Vec<Value> {
    let mut defs = definitions();
    for def in provider.map(ToolProvider::definitions).unwrap_or_default() {
        let name = tool_name(&def);
        // The plugin registry refuses such plugins already; this only keeps the list unambiguous.
        if !name.is_empty() && !defs.iter().any(|d| tool_name(d) == name) {
            defs.push(def);
        }
    }
    defs
}

/// [`writes`] for the fixed tools and those of `provider`.
pub fn writes_with(provider: Option<&dyn ToolProvider>, name: &str) -> bool {
    if is_fixed(name) {
        return writes(name);
    }
    provider.and_then(|p| p.writes(name)).unwrap_or(false)
}

/// [`call`] for the fixed tools and those of `provider`.
pub fn call_with(
    cx: &Context,
    store: &mut ProposalStore,
    origin: &Origin,
    provider: Option<&dyn ToolProvider>,
    name: &str,
    arguments: Value,
) -> ToolResult<Value> {
    match provider {
        Some(p) if !is_fixed(name) && p.writes(name).is_some() => p.call(cx, store, origin, name, arguments),
        _ => call(cx, store, origin, name, arguments),
    }
}

#[cfg(feature = "schema")]
fn schema<T: schemars::JsonSchema>() -> Value {
    let mut v = serde_json::to_value(schemars::schema_for!(T)).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("$schema");
        o.remove("title");
    }
    v
}

/// The tool definitions for an OpenAI-compatible chat request.
#[cfg(feature = "schema")]
pub fn definitions() -> Vec<Value> {
    let none = || json!({"type": "object", "properties": {}});
    TOOLS
        .iter()
        .map(|(name, description, _)| {
            let parameters = match *name {
                "search_models" => schema::<ModelFilter>(),
                "get_model" | "folder_summary" => schema::<IdArgs>(),
                "check_printability" => schema::<IdsArgs>(),
                "list_inbox" => schema::<InboxArgs>(),
                "get_list" => schema::<ListArgs>(),
                "find_duplicates" => schema::<DuplicatesArgs>(),
                "propose_tags" => schema::<proposals::ProposeTags>(),
                "propose_note" => schema::<proposals::ProposeNote>(),
                "propose_list" => schema::<proposals::ProposeList>(),
                "propose_tag_merge" => schema::<proposals::ProposeTagMerge>(),
                "propose_moves" => schema::<proposals::ProposeMoves>(),
                _ => none(),
            };
            json!({"type": "function", "function": {"name": name, "description": description, "parameters": parameters}})
        })
        .collect()
}

fn args<T: serde::de::DeserializeOwned>(value: Value) -> ToolResult<T> {
    // Models send `{}` or nothing for tools without parameters, and sometimes a JSON string.
    let value = match value {
        Value::Null => json!({}),
        Value::String(s) => serde_json::from_str(&s).unwrap_or_else(|_| json!({})),
        v => v,
    };
    serde_json::from_value(value).map_err(|e| ToolError::Invalid(format!("arguments: {e}")))
}

fn to_value<T: serde::Serialize>(value: T) -> ToolResult<Value> {
    serde_json::to_value(value).map_err(|e| ToolError::Invalid(e.to_string()))
}

/// Calls tool `name`; suggestions go to `store` as coming from `origin`.
pub fn call(
    cx: &Context,
    store: &mut ProposalStore,
    origin: &Origin,
    name: &str,
    arguments: Value,
) -> ToolResult<Value> {
    match name {
        "library_overview" => to_value(read::library_overview(cx)?),
        "search_models" => to_value(read::search_models(cx, &args::<ModelFilter>(arguments)?)?),
        "get_model" => to_value(read::get_model(cx, &args::<IdArgs>(arguments)?.id, false)?.0),
        "folder_summary" => to_value(read::folder_summary(cx, &args::<IdArgs>(arguments)?.id)?),
        "list_inbox" => to_value(read::list_inbox(cx, args::<InboxArgs>(arguments)?.folder.as_deref(), 200)?),
        "list_tags" => to_value(read::list_tags(cx)?),
        "list_lists" => to_value(read::list_lists(cx)?),
        "get_list" => to_value(read::get_list(cx, &args::<ListArgs>(arguments)?.name)?),
        "find_duplicates" => {
            to_value(read::find_duplicates(cx, args::<DuplicatesArgs>(arguments)?.library.as_deref(), 50)?)
        }
        "list_printers" => to_value(read::list_printers(cx)),
        "check_printability" => to_value(read::check_printability(cx, &args::<IdsArgs>(arguments)?.ids)?),
        "propose_tags" => to_value(proposals::propose_tags(cx, store, origin, &args(arguments)?)?),
        "propose_note" => to_value(proposals::propose_note(cx, store, origin, &args(arguments)?)?),
        "propose_list" => to_value(proposals::propose_list(cx, store, origin, &args(arguments)?)?),
        "propose_tag_merge" => to_value(proposals::propose_tag_merge(cx, store, origin, &args(arguments)?)?),
        "propose_moves" => to_value(proposals::propose_moves(cx, store, origin, &args(arguments)?)?),
        other => Err(ToolError::Invalid(format!("no tool called {other}"))),
    }
}

#[cfg(all(test, feature = "schema"))]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_definition_with_parameters() {
        let defs = super::definitions();
        assert_eq!(defs.len(), super::TOOLS.len());
        let search = defs.iter().find(|d| d["function"]["name"] == "search_models").unwrap();
        assert!(search["function"]["parameters"]["properties"]["fits_printer"].is_object());
        assert!(super::writes("propose_tags") && !super::writes("search_models"));
    }

    /// A plugin with a reading and a writing tool, and one named like a fixed tool.
    struct Plugin;

    impl ToolProvider for Plugin {
        fn definitions(&self) -> Vec<Value> {
            ["hello-box_measure", "hello-box_make_box", "library_overview"]
                .iter()
                .map(|n| json!({"type": "function", "function": {"name": n, "description": "", "parameters": {}}}))
                .collect()
        }

        fn writes(&self, name: &str) -> Option<bool> {
            match name {
                "hello-box_measure" => Some(false),
                "hello-box_make_box" | "library_overview" => Some(true),
                _ => None,
            }
        }

        fn call(&self, _: &Context, _: &mut ProposalStore, _: &Origin, name: &str, _: Value) -> ToolResult<Value> {
            Ok(json!({ "called": name }))
        }
    }

    #[test]
    fn provided_tools_come_after_the_fixed_ones_and_never_replace_them() {
        let defs = definitions_with(Some(&Plugin));
        let names: Vec<&str> = defs.iter().map(tool_name).collect();
        assert_eq!(names.len(), TOOLS.len() + 2, "{names:?}");
        assert_eq!(names[TOOLS.len()..], ["hello-box_measure", "hello-box_make_box"]);
        assert_eq!(names.iter().filter(|n| **n == "library_overview").count(), 1);
        assert_eq!(definitions_with(None).len(), TOOLS.len());

        assert!(writes_with(Some(&Plugin), "hello-box_make_box"));
        assert!(!writes_with(Some(&Plugin), "hello-box_measure"));
        assert!(!writes_with(Some(&Plugin), "library_overview"), "the fixed tool counts");
        assert!(writes_with(Some(&Plugin), "propose_tags"));
        assert!(!writes_with(None, "hello-box_make_box") && !writes_with(Some(&Plugin), "nothing"));
    }

    #[test]
    fn calls_go_to_the_provider_only_for_its_own_tools() {
        let lib = crate::db::Library::open_in_memory().unwrap();
        let config = crate::config::Config {
            ai: crate::config::AiConfig { enabled: true, ..Default::default() },
            ..Default::default()
        };
        let cx = Context::new(&lib, &config, crate::config::AccessClass::Local, None).unwrap();
        let mut store = ProposalStore::open_in_memory().unwrap();
        let origin = Origin { source: "chat".into(), client: None, class: crate::config::AccessClass::Local };
        let call =
            |name: &str, store: &mut ProposalStore| call_with(&cx, store, &origin, Some(&Plugin), name, json!({}));
        assert_eq!(call("hello-box_make_box", &mut store).unwrap(), json!({"called": "hello-box_make_box"}));
        assert!(call("library_overview", &mut store).unwrap().get("called").is_none(), "the fixed tool answers");
        assert!(call("hello-box_nothing", &mut store).unwrap_err().to_string().contains("no tool called"));
    }
}
