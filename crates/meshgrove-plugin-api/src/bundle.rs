//! Files a plugin hands over for the library: what it wrote below `/out`, where that goes, how
//! it is tagged and which lists it joins. Meshgrove applies a bundle as one change the user can
//! undo; the files never replace anything there.
//!
//! They go into a folder of their own ([`Bundle::folder`]), or, as loose files, beside the
//! library files they were made from ([`Bundle::next_to`], [`BundleFile::next_to`]).

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
    /// A library file (its id, as `Event::ModelsOpened` or `library.search` gave it) the files
    /// are put beside: loose, each under its own name in that file's folder (the folder of the
    /// archive, for a file inside one), with a number where the name is taken (" (2)"). Not
    /// together with `folder`. A file may name another one ([`BundleFile::next_to`]); all of
    /// them have to be in one library. The plugin has to be able to read the file: the user
    /// opened it with the panel, or lets the plugin read the library. From API 0.2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_to: Option<String>,
    /// Work Meshgrove is to start to make the bundle's files (a tool that takes too long to
    /// answer with them). From API 0.2; this Meshgrove refuses such a bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<BundleJob>,
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
    /// The library file this one is put beside, where that is not the bundle's
    /// ([`Bundle::next_to`]): one bundle for several sources. From API 0.2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_to: Option<String>,
}

/// See [`Bundle::job`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BundleJob {
    /// What `run-job` gets as `kind`.
    pub kind: String,
    #[serde(default)]
    pub params: serde_json::Value,
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
    /// Whether the files land loose beside library files, not in a folder of their own.
    pub fn uses_next_to(&self) -> bool {
        self.next_to.is_some() || self.files.iter().any(|f| f.next_to.is_some())
    }

    /// The library files the bundle's files are put beside, each once.
    pub fn sources(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for id in self.next_to.iter().chain(self.files.iter().filter_map(|f| f.next_to.as_ref())) {
            if !out.contains(&id.as_str()) {
                out.push(id);
            }
        }
        out
    }

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
            files: vec![BundleFile { path: "box.stl".into(), tags: vec!["Box".into()], ..BundleFile::default() }],
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
        assert!(!b.uses_next_to() && b.sources().is_empty());
        assert!(json.get("next_to").is_none() && json["files"][0].get("next_to").is_none(), "{json}");
    }

    #[test]
    fn a_bundle_names_the_files_its_own_go_beside() {
        let file = |path: &str, next_to: Option<&str>| BundleFile {
            path: path.into(),
            next_to: next_to.map(str::to_owned),
            ..BundleFile::default()
        };
        let b = Bundle {
            title: "Textured".into(),
            next_to: Some("Garten/rose.stl".into()),
            files: vec![file("rose – wood.3mf", None), file("topf – wood.3mf", Some("Garten/Töpfe/topf.stl"))],
            ..Bundle::default()
        };
        assert!(b.uses_next_to());
        assert_eq!(b.sources(), ["Garten/rose.stl", "Garten/Töpfe/topf.stl"]);
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(json["next_to"], "Garten/rose.stl");
        assert_eq!(json["files"][1]["next_to"], "Garten/Töpfe/topf.stl");
        assert_eq!(serde_json::from_value::<Bundle>(json).unwrap(), b);
        let only_a_file = Bundle { next_to: None, ..b.clone() };
        assert!(only_a_file.uses_next_to());
        assert_eq!(only_a_file.sources(), ["Garten/Töpfe/topf.stl"]);
        let job: Bundle =
            serde_json::from_str(r#"{"title": "T", "job": {"kind": "export", "params": {"n": 1}}}"#).unwrap();
        assert_eq!(job.job.unwrap().kind, "export");
    }
}
