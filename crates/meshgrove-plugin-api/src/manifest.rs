//! What a plugin says about itself, and the tools it offers the AI.

use serde::{Deserialize, Serialize};

/// Most tools one plugin may offer.
pub const MAX_TOOLS: usize = 8;
/// Largest input schema of one tool, as JSON.
pub const MAX_SCHEMA_BYTES: usize = 8 * 1024;
/// Longest tool name as the AI sees it (`<plugin>_<tool>`); what AI clients accept.
pub const MAX_TOOL_NAME: usize = 64;

/// The answer of `plugin.manifest()`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Manifest {
    /// `[a-z][a-z0-9-]{1,30}`, unique among the installed plugins ("bingrove", "hello-box").
    pub id: String,
    /// The name shown in Meshgrove ("Gridfinity").
    pub name: String,
    /// The plugin's own version.
    pub version: String,
    /// The API version it was built against ([`crate::API_VERSION`]).
    pub api: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// What it asks for; the user grants each in Meshgrove's settings.
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub panels: Vec<PanelDef>,
    /// Files Meshgrove offers to open with the plugin.
    #[serde(default)]
    pub file_types: Vec<FileType>,
    /// Memory it needs, in MiB; the user's setting wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Its tools are offered to the AI (the chat, `meshgrove-mcp`).
    AiTools,
    /// It may hand over bundles of files for the library.
    Bundles,
    /// It may read the library (search, facts, triangles) through `library`.
    LibraryRead,
    /// One this Meshgrove does not know; it is never granted.
    #[serde(other)]
    Unknown,
}

/// A panel the plugin can open in Meshgrove's window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PanelDef {
    pub id: String,
    /// The tab's title.
    pub title: String,
}

/// Files the plugin opens: `*.gridfinity.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FileType {
    /// A file name pattern, `*` for any characters; case does not matter.
    pub pattern: String,
    #[serde(default)]
    pub description: String,
    /// The entry in the gallery's context menu and the button in the Info pane ("Apply a
    /// texture …"); none: "Open with <plugin>". From API 0.2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// What the plugin gets of the files. From API 0.2.
    #[serde(default, skip_serializing_if = "Receive::is_bytes")]
    pub receive: Receive,
    /// With [`Receive::Ids`]: the panel (one of the manifest's) the files are opened in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<String>,
}

/// How files opened with a plugin reach it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Receive {
    /// One file, its name and bytes, through `open-file`.
    #[default]
    Bytes,
    /// The files' library ids, as [`crate::ui::Event::ModelsOpened`] to [`FileType::panel`]: one
    /// file or a whole selection. The panel and its jobs may read exactly these files through
    /// `library`, with or without the `library_read` permission.
    Ids,
}

impl Receive {
    fn is_bytes(&self) -> bool {
        *self == Receive::Bytes
    }
}

impl FileType {
    pub fn matches(&self, file_name: &str) -> bool {
        glob(&self.pattern.to_lowercase(), &file_name.to_lowercase())
    }
}

fn glob(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, rest)) => {
            let Some(name) = name.strip_prefix(head) else { return false };
            if rest.is_empty() {
                return true;
            }
            (0..=name.len()).filter(|&i| name.is_char_boundary(i)).any(|i| glob(rest, &name[i..]))
        }
    }
}

/// One tool for the AI. Meshgrove names it `<plugin id>_<name>`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ToolDef {
    /// `[a-z][a-z0-9_]*`.
    pub name: String,
    /// What it does and when to use it, for the AI.
    pub description: String,
    /// JSON schema (an object) of its arguments.
    pub input_schema: serde_json::Value,
    /// It may return a bundle (a change of the library, which the user may have to accept).
    #[serde(default)]
    pub bundles: bool,
}

/// The answer of `host.info()`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HostInfo {
    /// "de" or "en".
    pub language: String,
    pub dark: bool,
    /// The user's printers, the main one first.
    #[serde(default)]
    pub printers: Vec<PrinterInfo>,
    /// Meshgrove's version.
    #[serde(default)]
    pub meshgrove: String,
    /// The API version of the host.
    #[serde(default)]
    pub api: String,
    /// The user reads lengths in inches (millimetres otherwise). Lengths cross the interface in
    /// millimetres either way; this is for texts the plugin writes itself. From API 0.2.
    #[serde(default)]
    pub imperial: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PrinterInfo {
    pub name: String,
    /// Build volume in mm (x, y, z).
    pub bed: [f32; 3],
    /// Nozzle diameter in mm, when the user set one. From API 0.2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle: Option<f32>,
}

/// `[a-z][a-z0-9-]{1,30}`.
pub fn valid_plugin_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && (2..=31).contains(&id.len())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `[a-z][a-z0-9_]*`.
pub fn valid_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A short id of a panel, job or widget: `[A-Za-z0-9_.-]`, 1 to 64 characters.
fn valid_local_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

impl Manifest {
    /// Whether the manifest is usable on this host; the text says what is wrong.
    pub fn check(&self) -> Result<(), String> {
        self.check_for(crate::API_VERSION)
    }

    /// [`Manifest::check`] for a host of API `host`.
    pub fn check_for(&self, host: &str) -> Result<(), String> {
        if !valid_plugin_id(&self.id) {
            return Err(format!("plugin id {:?} is not of the form [a-z][a-z0-9-]{{1,30}}", self.id));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 60 {
            return Err("the name is empty or longer than 60 characters".into());
        }
        if self.version.trim().is_empty() {
            return Err("the version is empty".into());
        }
        crate::api_check(host, &self.api)?;
        for (i, panel) in self.panels.iter().enumerate() {
            if !valid_local_id(&panel.id) || self.panels[..i].iter().any(|p| p.id == panel.id) {
                return Err(format!("panel id {:?} is invalid or used twice", panel.id));
            }
        }
        if let Some(f) = self.file_types.iter().find(|f| f.pattern.trim().is_empty() || f.pattern == "*") {
            return Err(format!("file type pattern {:?} is too wide", f.pattern));
        }
        for f in self.file_types.iter().filter(|f| f.receive == Receive::Ids) {
            if !f.panel.as_ref().is_some_and(|id| self.panels.iter().any(|p| p.id == *id)) {
                return Err(format!("file type {:?} receives ids, so it must name one of the panels", f.pattern));
            }
        }
        Ok(())
    }

    /// The file type a selection of files (by their names) opens with, if the plugin opens it:
    /// for one file the first type that matches; for several, each file's first match must
    /// take ids ([`Receive::Ids`]) in one and the same panel.
    pub fn opener(&self, names: &[&str]) -> Option<&FileType> {
        let of = |name: &str| self.file_types.iter().find(|t| t.matches(name));
        let (first, rest) = names.split_first()?;
        let opener = of(first)?;
        if rest.is_empty() {
            return Some(opener);
        }
        let same = |t: &FileType| t.receive == Receive::Ids && t.panel == opener.panel;
        (same(opener) && rest.iter().all(|n| of(n).is_some_and(same))).then_some(opener)
    }

    pub fn wants(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }
}

/// Whether the tools of plugin `id` are usable: at most [`MAX_TOOLS`], names valid and unique,
/// schemas objects of at most [`MAX_SCHEMA_BYTES`].
pub fn check_tools(id: &str, tools: &[ToolDef]) -> Result<(), String> {
    if tools.len() > MAX_TOOLS {
        return Err(format!("{} tools, at most {MAX_TOOLS} are allowed", tools.len()));
    }
    for (i, tool) in tools.iter().enumerate() {
        if !valid_tool_name(&tool.name) || id.len() + 1 + tool.name.len() > MAX_TOOL_NAME {
            return Err(format!("tool name {:?} is invalid or too long", tool.name));
        }
        if tools[..i].iter().any(|t| t.name == tool.name) {
            return Err(format!("tool {:?} is listed twice", tool.name));
        }
        if !tool.input_schema.is_object() {
            return Err(format!("the input schema of {:?} is not an object", tool.name));
        }
        if tool.input_schema.to_string().len() > MAX_SCHEMA_BYTES {
            return Err(format!("the input schema of {:?} is larger than {MAX_SCHEMA_BYTES} bytes", tool.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            id: "hello-box".into(),
            name: "Hello Box".into(),
            version: "1.0.0".into(),
            api: crate::API_VERSION.into(),
            permissions: vec![Permission::AiTools, Permission::Bundles],
            panels: vec![PanelDef { id: "box".into(), title: "Box".into() }],
            file_types: vec![FileType { pattern: "*.box.json".into(), ..FileType::default() }],
            ..Manifest::default()
        }
    }

    #[test]
    fn manifests_round_trip_and_are_checked() {
        let m = manifest();
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Manifest>(&json).unwrap(), m);
        assert_eq!(m.check(), Ok(()));
        assert!(Manifest { id: "Hello".into(), ..manifest() }.check().is_err());
        assert!(Manifest { id: "h".into(), ..manifest() }.check().is_err());
        assert!(Manifest { api: "0.9.0".into(), ..manifest() }.check().unwrap_err().contains("0.9.0"));
        assert_eq!(Manifest { api: "0.1.0".into(), ..manifest() }.check(), Ok(()), "a 0.1 plugin still runs");
        let newer = manifest().check_for("0.1.0").unwrap_err();
        assert!(newer.contains("update Meshgrove"), "a 0.2 plugin on a 0.1 host: {newer}");
        let one = Manifest { api: "1.0.0".into(), ..manifest() };
        assert!(one.check_for("0.2.0").unwrap_err().contains("built for plugin API 1.0.0"));
        assert_eq!(manifest().check_for("0.3.0"), Ok(()), "a host runs plugins of its own API");
        let older = Manifest { api: "0.2.0".into(), ..manifest() };
        assert_eq!(older.check_for("0.3.0"), Ok(()), "a 0.3 host runs 0.2 plugins");
        assert!(manifest().check_for("0.2.0").unwrap_err().contains("update Meshgrove"), "a 0.3 plugin on 0.2");
        let twice = vec![PanelDef { id: "a".into(), title: "A".into() }; 2];
        assert!(Manifest { panels: twice, ..manifest() }.check().is_err());
        let unknown: Manifest = serde_json::from_str(
            r#"{"id":"x-y","name":"X","version":"1","api":"0.1.0","permissions":["ai_tools","teleport"]}"#,
        )
        .unwrap();
        assert_eq!(unknown.permissions, [Permission::AiTools, Permission::Unknown]);
    }

    #[test]
    fn the_host_says_units_and_nozzles() {
        let info = HostInfo {
            language: "en".into(),
            imperial: true,
            printers: vec![
                PrinterInfo { name: "Kobra".into(), bed: [250.0, 250.0, 260.0], nozzle: Some(0.4) },
                PrinterInfo { name: "Old".into(), bed: [200.0, 200.0, 180.0], nozzle: None },
            ],
            ..HostInfo::default()
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(
            (json["imperial"].as_bool(), json["printers"][0]["nozzle"].as_f64()),
            (Some(true), Some(0.4f32 as f64))
        );
        assert!(json["printers"][1].get("nozzle").is_none());
        assert_eq!(serde_json::from_value::<HostInfo>(json).unwrap(), info);
        // What a Meshgrove 0.8 says: metric, no nozzles.
        let old: HostInfo = serde_json::from_str(
            r#"{"language":"de","dark":false,"printers":[{"name":"Kobra","bed":[250,250,260]}],"meshgrove":"0.8.3","api":"0.1.0"}"#,
        )
        .unwrap();
        assert_eq!((old.imperial, old.printers[0].nozzle), (false, None));
    }

    #[test]
    fn file_types_match_by_pattern() {
        let t = FileType { pattern: "*.Gridfinity.json".into(), ..FileType::default() };
        assert!(t.matches("Schublade.gridfinity.json") && !t.matches("x.json"));
        let t = FileType { pattern: "plan-*.txt".into(), ..FileType::default() };
        assert!(t.matches("plan-1.txt") && !t.matches("xplan-1.txt"));
    }

    #[test]
    fn file_types_default_to_bytes_and_ids_need_a_panel() {
        // What an API 0.1 plugin says: the file's bytes, the usual menu entry.
        let old: FileType = serde_json::from_str(r#"{"pattern": "*.box.json", "description": "A saved box"}"#).unwrap();
        assert_eq!((old.receive, &old.label, &old.panel), (Receive::Bytes, &None, &None));
        let json = serde_json::to_value(&old).unwrap();
        assert_eq!(json, serde_json::json!({"pattern": "*.box.json", "description": "A saved box"}), "nothing new");
        let ids = FileType {
            pattern: "*.stl".into(),
            label: Some("Measure …".into()),
            receive: Receive::Ids,
            panel: Some("box".into()),
            ..FileType::default()
        };
        let json = serde_json::to_value(&ids).unwrap();
        assert_eq!((json["receive"].as_str(), json["panel"].as_str()), (Some("ids"), Some("box")));
        assert_eq!(serde_json::from_value::<FileType>(json).unwrap(), ids);
        assert_eq!(Manifest { file_types: vec![ids.clone()], ..manifest() }.check(), Ok(()));
        for panel in [None, Some("nope".to_owned())] {
            let bad = Manifest { file_types: vec![FileType { panel, ..ids.clone() }], ..manifest() };
            assert!(bad.check().unwrap_err().contains("must name one of the panels"));
        }
    }

    #[test]
    fn a_selection_opens_when_every_file_fits() {
        let ids = |pattern: &str, panel: &str| FileType {
            pattern: pattern.into(),
            receive: Receive::Ids,
            panel: Some(panel.into()),
            ..FileType::default()
        };
        let m = Manifest {
            file_types: vec![
                FileType { pattern: "*.box.json".into(), ..FileType::default() },
                ids("*.stl", "box"),
                ids("*.3mf", "box"),
                ids("*.step", "other"),
            ],
            ..manifest()
        };
        assert_eq!(m.opener(&[]), None);
        assert_eq!(m.opener(&["a.box.json"]).map(|t| t.receive), Some(Receive::Bytes));
        assert_eq!(m.opener(&["A.STL"]).map(|t| t.pattern.as_str()), Some("*.stl"));
        assert_eq!(m.opener(&["a.stl", "b.3mf", "c.stl"]).map(|t| t.pattern.as_str()), Some("*.stl"));
        assert_eq!(m.opener(&["a.stl", "notes.txt"]), None, "one file does not fit");
        assert_eq!(m.opener(&["a.box.json", "b.box.json"]), None, "bytes are one file at a time");
        assert_eq!(m.opener(&["a.stl", "b.step"]), None, "two panels");
        assert_eq!(m.opener(&["a.txt"]), None);
    }

    #[test]
    fn tools_are_limited() {
        let tool = |name: &str| ToolDef {
            name: name.into(),
            description: "d".into(),
            input_schema: serde_json::json!({"type": "object"}),
            bundles: false,
        };
        assert_eq!(check_tools("hello-box", &[tool("make_box")]), Ok(()));
        assert!(check_tools("hello-box", &[tool("Make")]).is_err());
        assert!(check_tools("hello-box", &[tool("a"), tool("a")]).is_err());
        assert!(check_tools("hello-box", &(0..9).map(|i| tool(&format!("t{i}"))).collect::<Vec<_>>()).is_err());
        let big = ToolDef { input_schema: serde_json::json!({"description": "x".repeat(9000)}), ..tool("big") };
        assert!(check_tools("hello-box", &[big]).is_err());
    }
}
