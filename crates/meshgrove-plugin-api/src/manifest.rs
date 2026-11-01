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
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PrinterInfo {
    pub name: String,
    /// Build volume in mm (x, y, z).
    pub bed: [f32; 3],
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
        Ok(())
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
            file_types: vec![FileType { pattern: "*.box.json".into(), description: String::new() }],
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
        let one = Manifest { api: "1.0.0".into(), ..manifest() };
        assert!(one.check_for("0.2.0").unwrap_err().contains("built for plugin API 1.0.0"));
        assert_eq!(manifest().check_for("0.2.0"), Ok(()), "a 0.2 host runs 0.1 plugins");
        let twice = vec![PanelDef { id: "a".into(), title: "A".into() }; 2];
        assert!(Manifest { panels: twice, ..manifest() }.check().is_err());
        let unknown: Manifest = serde_json::from_str(
            r#"{"id":"x-y","name":"X","version":"1","api":"0.1.0","permissions":["ai_tools","teleport"]}"#,
        )
        .unwrap();
        assert_eq!(unknown.permissions, [Permission::AiTools, Permission::Unknown]);
    }

    #[test]
    fn file_types_match_by_pattern() {
        let t = FileType { pattern: "*.Gridfinity.json".into(), description: String::new() };
        assert!(t.matches("Schublade.gridfinity.json") && !t.matches("x.json"));
        let t = FileType { pattern: "plan-*.txt".into(), description: String::new() };
        assert!(t.matches("plan-1.txt") && !t.matches("xplan-1.txt"));
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
