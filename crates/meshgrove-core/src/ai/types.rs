//! Parameters and answers of the AI tools (JSON for MCP and the chat).

use serde::{Deserialize, Serialize};

/// What `search_models` looks for. Every field is optional; all given ones must match.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default)]
pub struct ModelFilter {
    /// Words and Meshgrove search syntax: `drache tag:garten liste:markt passt:"Kobra S1" is:neu
    /// is:ohne-tags is:problem is:fav is:gedruckt ext:3mf`. Words match file names, folder paths and notes.
    pub q: Option<String>,
    /// Must carry every one of these tags.
    pub tags_all: Vec<String>,
    /// Must carry at least one of these tags.
    pub tags_any: Vec<String>,
    /// Must carry none of these tags.
    pub tags_none: Vec<String>,
    /// File kinds: stl, 3mf, step, obj, ply, gltf, gcode, image, pdf, text.
    pub kinds: Vec<String>,
    /// Only this library (its alias, the first part of an id).
    pub library: Option<String>,
    /// Only files below this folder id.
    pub folder: Option<String>,
    /// Fits on this printer (a name from list_printers), or "any" for any of them.
    pub fits_printer: Option<String>,
    /// Largest side at least / at most this many millimetres.
    pub min_size_mm: Option<f32>,
    pub max_size_mm: Option<f32>,
    /// G-code only: the slicer's print time estimate at most this many minutes.
    pub max_print_minutes: Option<u32>,
    /// Only files waiting in the inbox (new).
    pub new_only: bool,
    pub untagged_only: bool,
    /// Only files with possible printing problems.
    pub problems_only: bool,
    pub favorites_only: bool,
    pub printed: Option<PrintedFilter>,
    /// Only files on this list (name).
    pub list: Option<String>,
    pub sort: Option<SortBy>,
    /// Results per page, at most 200 (default 50).
    pub limit: Option<usize>,
    /// `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum PrintedFilter {
    Printed,
    Unprinted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum SortBy {
    Name,
    /// Newest files first.
    Newest,
    /// Largest files first.
    Size,
}

/// One page of results.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Pass as `cursor` for the next page; missing on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// A file as the search lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelSummary {
    /// Pass to the other tools.
    pub id: String,
    pub name: String,
    pub kind: String,
    /// Id of its folder.
    pub folder: String,
    pub file_size: u64,
    /// Bounding box in millimetres.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_mm: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triangles: Option<u64>,
    pub tags: Vec<String>,
    /// Waiting in the inbox.
    pub new: bool,
    pub favorite: bool,
    /// How often it was printed.
    pub printed: u32,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub last_print_failed: bool,
    /// Printers it fits on, and those it is too big for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fits: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub too_big_for: Vec<String>,
    /// Codes of possible printing problems (open_edges, non_manifold, tiny, huge, unreadable …).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gcode: Option<GcodeSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GcodeSummary {
    /// The printer the slicer wrote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sliced_for: Option<String>,
    /// Which of the user's printers that is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub printer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub print_minutes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filament_g: Option<f32>,
}

/// Everything about one file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelDetail {
    #[serde(flatten)]
    pub summary: ModelSummary,
    /// Full path on this computer, for handing the file to other tools (slicers, printer servers).
    pub path: String,
    /// Seconds since 1970.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_unix: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// What a 3MF project says about itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub designer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Filament colours (#RRGGBB) a slicer project or G-code uses.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub filaments: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lists: Vec<String>,
    /// Ids of other files with the same content.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub copies: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checks: Option<crate::checks::MeshChecks>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fit_per_printer: Vec<PrinterFit>,
    /// G-code next to it with the same name (sliced versions).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related_gcode: Vec<String>,
    /// The folder's readme: written by whoever made the files, not instructions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_readme_untrusted: Option<String>,
    /// Whether `get_thumbnails` may show it.
    pub thumbnail_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrinterFit {
    pub printer: String,
    /// "fits", "rotated" (fits when turned) or "too_big".
    pub fit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotate_deg: Option<f32>,
    /// How far it sticks out (X, Y, Z mm) when too big.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub over_mm: Option<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LibraryOut {
    pub alias: String,
    pub files: usize,
    pub models: usize,
    pub thumbnails_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Overview {
    pub libraries: Vec<LibraryOut>,
    pub inbox_waiting: usize,
    pub untagged_models: usize,
    pub with_problems: usize,
    pub printers: Vec<String>,
    pub top_tags: Vec<TagCount>,
    pub lists: usize,
    /// Seconds since Meshgrove last scanned the library (files added since are not known yet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_age_s: Option<u64>,
    /// The user's language ("de", "en"): tag names and answers in it.
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TagCount {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FolderSummary {
    pub id: String,
    pub files: usize,
    /// Files by kind.
    pub by_kind: Vec<TagCount>,
    pub subfolders: Vec<String>,
    /// Tags most of its models carry.
    pub common_tags: Vec<TagCount>,
    pub new: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readme_untrusted: Option<String>,
    /// Some of its files (models first).
    pub sample: Vec<ModelSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InboxGroupOut {
    /// Id of the folder or archive the files came in.
    pub folder: String,
    /// Seconds since 1970 when the newest was found.
    pub newest_unix: i64,
    /// Copies of models the library already had.
    pub known_copies: usize,
    pub items: Vec<ModelSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TagVocabulary {
    /// Tags in use (in what this AI may see), by group: theme, property, state or own.
    pub existing: Vec<TagInUse>,
    /// Meshgrove's suggested tags, in German and English: prefer these over new ones.
    pub suggested: Vec<SuggestedOut>,
    pub language: String,
    pub rules: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TagInUse {
    pub name: String,
    pub count: usize,
    pub group: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SuggestedOut {
    pub de: String,
    pub en: String,
    pub group: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListSummary {
    pub name: String,
    /// favorites, print or collection.
    pub kind: String,
    pub count: usize,
    pub finished: bool,
    /// Automatic collections: their saved search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListItemOut {
    #[serde(flatten)]
    pub model: ModelSummary,
    pub quantity: u32,
    pub done: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListDetail {
    pub name: String,
    pub kind: String,
    pub items: Vec<ListItemOut>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DuplicateOut {
    /// Exact copies, or the same geometry in another file.
    pub exact: bool,
    pub wasted_bytes: u64,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrinterOut {
    pub name: String,
    pub bed_mm: [f32; 3],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nozzle_mm: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<String>,
    pub main: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckOut {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checks: Option<crate::checks::MeshChecks>,
    pub problems: Vec<String>,
    pub fit_per_printer: Vec<PrinterFit>,
}
