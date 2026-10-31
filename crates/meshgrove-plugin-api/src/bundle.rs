//! Files a plugin hands over for the library: what it wrote below `/out`, where that goes, how
//! it is tagged and which lists it joins. Meshgrove applies a bundle as one change the user can
//! undo; the files never replace anything there.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Bundle {
    /// For the history and the review ("Gridfinity 250 × 305").
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The folder below the library the files land in, `/`-separated ("Gridfinity/Schublade
    /// 250x305"); taken already, it gets a number (" (2)"). None: a folder named after the
    /// title in the library's import folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Files below `/out` with their own tags and notes. Files not listed come along as well.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<BundleFile>,
    /// Tags for every file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lists: Vec<BundleList>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BundleFile {
    /// Below `/out`, `/`-separated ("parts/bin_2x2.stl").
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A list (or the existing one of that kind and name) the files join.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BundleList {
    pub name: String,
    #[serde(default)]
    pub kind: ListKind,
    pub items: Vec<BundleItem>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    /// What to print, how many of each.
    #[default]
    Print,
    Collection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BundleItem {
    /// A file below `/out`, as in [`BundleFile::path`].
    pub path: String,
    #[serde(default = "one")]
    pub quantity: u32,
}

fn one() -> u32 {
    1
}

impl Bundle {
    /// Every path the bundle names (files and list items), each once.
    pub fn paths(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        let named = self.files.iter().map(|f| f.path.as_str());
        for p in named.chain(self.lists.iter().flat_map(|l| l.items.iter().map(|i| i.path.as_str()))) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_round_trip_with_defaults() {
        let b = Bundle {
            title: "Box".into(),
            folder: Some("Hello Box/box".into()),
            files: vec![BundleFile { path: "box.stl".into(), tags: vec!["Box".into()], note: None }],
            tags: vec!["hello-box".into()],
            lists: vec![BundleList {
                name: "Boxes".into(),
                kind: ListKind::Print,
                items: vec![BundleItem { path: "box.stl".into(), quantity: 2 }],
            }],
            ..Bundle::default()
        };
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(serde_json::from_value::<Bundle>(json.clone()).unwrap(), b);
        assert!(json.get("summary").is_none(), "empty fields stay out");
        let short: Bundle =
            serde_json::from_str(r#"{"title":"T","lists":[{"name":"L","items":[{"path":"a.stl"}]}]}"#).unwrap();
        assert_eq!((short.lists[0].kind, short.lists[0].items[0].quantity), (ListKind::Print, 1));
        assert_eq!(b.paths(), ["box.stl"]);
    }
}
