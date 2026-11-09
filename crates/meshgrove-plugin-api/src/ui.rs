//! The panels of a plugin, Elm-like: the plugin renders a [`View`] (a tree of [`Node`]s),
//! Meshgrove draws it with its own widgets and sends back an [`Event`] when the user does
//! something; the plugin answers with [`Effect`]s and renders anew.
//!
//! Widgets Meshgrove does not know (from a newer API) are read as [`Node::Unknown`] and drawn
//! as a placeholder, so an older Meshgrove still shows the rest of the panel.

use serde::{Deserialize, Serialize};

use crate::bundle::Bundle;
use crate::manifest::HostInfo;

/// Version of the widget tree.
pub const UI_VERSION: u32 = 1;

/// What `panel.render()` returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct View {
    /// [`UI_VERSION`] when it was rendered.
    pub v: u32,
    pub root: Node,
}

impl View {
    pub fn new(root: Node) -> View {
        View { v: UI_VERSION, root }
    }
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn one() -> u32 {
    1
}

/// One widget. Every input carries an `id`; its events name it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    // ---- Layout ----
    Column {
        #[serde(default)]
        children: Vec<Node>,
    },
    Row {
        #[serde(default)]
        children: Vec<Node>,
    },
    /// A framed group, with a title that folds it when `collapsible`.
    Group {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        collapsible: bool,
        #[serde(default)]
        children: Vec<Node>,
    },
    Tabs {
        id: String,
        #[serde(default)]
        selected: usize,
        tabs: Vec<Tab>,
    },
    /// Two parts side by side (or one above the other when `vertical`), the user can move the
    /// border; `ratio` is the first part's share.
    Split {
        #[serde(default, skip_serializing_if = "is_false")]
        vertical: bool,
        #[serde(default = "half")]
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
    Scroll {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_height: Option<f32>,
        #[serde(default)]
        children: Vec<Node>,
    },

    // ---- Display ----
    Label {
        text: String,
        #[serde(default, skip_serializing_if = "TextStyle::is_body")]
        style: TextStyle,
    },
    Markdown {
        text: String,
    },
    Separator,
    /// A picture: PNG or JPEG, base64.
    Image {
        data: String,
        #[serde(default = "png")]
        mime: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alt: Option<String>,
    },
    Notice {
        #[serde(default)]
        level: Level,
        text: String,
    },
    /// `None`: busy, no idea how far.
    Progress {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fraction: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },

    // ---- Input ----
    Button {
        id: String,
        label: String,
        #[serde(default, skip_serializing_if = "is_false")]
        primary: bool,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        enabled: bool,
    },
    TextInput {
        id: String,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        multiline: bool,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        enabled: bool,
    },
    /// A number; with `unit` Mm Meshgrove shows it in the user's units (mm or inches) but always
    /// reports millimetres.
    NumberInput {
        id: String,
        value: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
        #[serde(default, skip_serializing_if = "Unit::is_none")]
        unit: Unit,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decimals: Option<u8>,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        enabled: bool,
    },
    Slider {
        id: String,
        value: f64,
        min: f64,
        max: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        enabled: bool,
    },
    Checkbox {
        id: String,
        label: String,
        checked: bool,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        enabled: bool,
    },
    /// A drop-down; `selected` is a choice's value.
    Select {
        id: String,
        options: Vec<Choice>,
        selected: String,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        enabled: bool,
    },
    Radio {
        id: String,
        options: Vec<Choice>,
        selected: String,
    },
    /// `#rrggbb` or `#rrggbbaa`.
    Color {
        id: String,
        value: String,
    },
    /// Labelled rows; with `submit` a button that sends [`Event::Submit`] with every value.
    Form {
        id: String,
        rows: Vec<FormRow>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        submit: Option<String>,
    },
    Table {
        id: String,
        columns: Vec<TableColumn>,
        rows: Vec<TableRow>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_height: Option<f32>,
    },

    // ---- Host widgets ----
    GridEditor(GridEditor),
    Canvas(Canvas),
    /// A 3D view of the plugin's triangles: when `revision` changes Meshgrove asks
    /// `panel.mesh(key)` for them anew.
    MeshView {
        id: String,
        key: String,
        revision: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<f32>,
        /// A printer bed to show under the model (x, y in mm).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bed: Option<[f32; 2]>,
        /// The space the model is meant to fit (its minimum and maximum corner, in the
        /// coordinates of the triangles), drawn as a coloured frame round the model and kept in
        /// the picture; a rectangle when it has no extent along one axis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<[[f32; 3]; 2]>,
    },

    /// A widget this Meshgrove does not know.
    #[serde(other)]
    Unknown,
}

fn half() -> f32 {
    0.5
}

fn png() -> String {
    "image/png".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Tab {
    pub label: String,
    #[serde(default)]
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TextStyle {
    #[default]
    Body,
    Heading,
    Strong,
    Small,
    Weak,
    Mono,
}

impl TextStyle {
    fn is_body(&self) -> bool {
        *self == TextStyle::Body
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Level {
    #[default]
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    #[default]
    None,
    /// A length in millimetres.
    Mm,
    Degrees,
    Percent,
}

impl Unit {
    fn is_none(&self) -> bool {
        *self == Unit::None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Choice {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FormRow {
    pub label: String,
    pub node: Node,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TableColumn {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
    /// Numbers right-aligned.
    #[serde(default, skip_serializing_if = "is_false")]
    pub numeric: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TableRow {
    pub id: String,
    pub cells: Vec<String>,
}

/// A grid of cells with items on it (Gridfinity bins in a drawer). Meshgrove handles the mouse
/// and keys itself (drag, resize, draw a new item, select several, arrows, Delete, a context
/// menu) with snapping and a red preview where something does not fit, and reports each change
/// as an [`Event::Grid`]; the plugin checks it and renders anew.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GridEditor {
    pub id: String,
    /// Cells across and down.
    pub cols: u32,
    pub rows: u32,
    /// Edge of one cell in mm (42 for Gridfinity).
    pub pitch: f32,
    /// Positions snap to this fraction of a cell (2: half cells).
    #[serde(default = "one")]
    pub snap_div: u32,
    /// Areas around or on the grid (the edges of a drawer), in cells; they may reach outside.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<GridZone>,
    #[serde(default)]
    pub items: Vec<GridItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GridZone {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Items may be placed into it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub accepts_items: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GridItem {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Short marks in a corner ("6u", "⚲").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub badges: Vec<String>,
    /// Entries of its context menu, reported as [`GridChange::Action`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Action {
    pub id: String,
    pub label: String,
}

/// A drawing in its own units (scaled to fit), for outlines and silhouettes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Canvas {
    pub id: String,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub shapes: Vec<Shape>,
    /// Clicks and drags are reported ([`Event::Canvas`]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub interactive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Shape {
    Line {
        points: Vec<[f32; 2]>,
        #[serde(default)]
        stroke: Stroke,
    },
    Polygon {
        points: Vec<[f32; 2]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<String>,
        #[serde(default)]
        stroke: Stroke,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<String>,
        #[serde(default)]
        stroke: Stroke,
        /// Named shapes are reported when clicked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Circle {
        x: f32,
        y: f32,
        r: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<String>,
        #[serde(default)]
        stroke: Stroke,
    },
    Text {
        x: f32,
        y: f32,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Stroke {
    pub width: f32,
    pub color: String,
}

impl Default for Stroke {
    fn default() -> Stroke {
        Stroke { width: 1.0, color: "#888888".into() }
    }
}

/// What the user did, or what happened to work the panel started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Click {
        id: String,
    },
    /// An input's new value (text, number in mm, bool, a choice's value, a tab's index).
    Changed {
        id: String,
        value: serde_json::Value,
    },
    /// A form's submit button: every input of the form by its id.
    Submit {
        id: String,
        values: serde_json::Map<String, serde_json::Value>,
    },
    TableSelect {
        id: String,
        row: Option<String>,
    },
    Grid {
        id: String,
        change: GridChange,
    },
    Canvas {
        id: String,
        change: CanvasChange,
    },
    JobProgress {
        job: String,
        fraction: f32,
        #[serde(default)]
        message: String,
    },
    /// A job is done: its answer, or why it failed.
    JobFinished {
        job: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A file the user picked after [`Effect::OpenFileDialog`], copied to `path` (below `/tmp`,
    /// or `/data/picked` when the effect asked for it); `name` is the file's own.
    FilePicked {
        id: String,
        name: String,
        path: String,
    },
    /// Language, theme or printers changed.
    HostChanged {
        info: HostInfo,
    },
    /// What became of a bundle the panel handed over with [`Effect::ApplyBundle`] or
    /// [`Effect::ProposeBundle`]. `title` is the bundle's. An older Meshgrove does not send it.
    BundleFinished {
        title: String,
        outcome: BundleOutcome,
    },
    /// Library files the user opened with this panel, by their ids (a file type that receives
    /// ids, [`crate::manifest::Receive::Ids`]): one file or a whole selection. The panel and the
    /// jobs it starts may read exactly these through `library` from now on, whether or not the
    /// user lets the plugin read the library. From API 0.2.
    ModelsOpened {
        ids: Vec<String>,
    },
    /// An event of a newer API than this crate's: nothing to do.
    #[serde(other)]
    Unknown,
}

/// See [`Event::BundleFinished`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BundleOutcome {
    /// In the library, as one change the history can undo: `steps` went through, `skipped` did
    /// nothing (a file already had that tag, say).
    Applied { steps: u32, skipped: u32 },
    /// Waiting in the review, for the user to decide.
    Queued,
    /// The user has not allowed this plugin to put files into the library (Settings → Plugins).
    /// The files are gone; nothing was tried.
    Refused,
    /// It did not work, and the library is as it was.
    Failed { error: String },
}

/// A change on a [`GridEditor`], in cells.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GridChange {
    Moved {
        item: String,
        x: f32,
        y: f32,
    },
    Resized {
        item: String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    /// The user drew a new item on free cells.
    Created {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    Merge {
        items: Vec<String>,
    },
    Split {
        item: String,
    },
    Deleted {
        items: Vec<String>,
    },
    Selected {
        items: Vec<String>,
    },
    /// An entry of the item's context menu.
    Action {
        item: String,
        action: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CanvasChange {
    Click {
        x: f32,
        y: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shape: Option<String>,
    },
    Drag {
        from: [f32; 2],
        to: [f32; 2],
    },
}

/// What the plugin asks Meshgrove to do after an event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    Toast {
        text: String,
        #[serde(default)]
        level: Level,
    },
    /// Background jobs, run in parallel, each in a fresh instance of the plugin; they all write
    /// into one staging folder (`/out`), which a following bundle hands over.
    StartJobs {
        jobs: Vec<JobSpec>,
    },
    CancelJob {
        job: String,
    },
    /// Put the files into the library now (the user clicked for it); undo is in the history.
    ApplyBundle {
        bundle: Bundle,
    },
    /// Let the user look at the bundle in the review first.
    ProposeBundle {
        bundle: Bundle,
    },
    /// Offer a file the plugin wrote (below `/out` or `/tmp`) for saving where the user likes.
    SaveFile {
        path: String,
        name: String,
    },
    /// Ask for a file with the system's file dialog; it arrives as [`Event::FilePicked`].
    OpenFileDialog {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Without the dot ("json", "png").
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extensions: Vec<String>,
        /// Several files may be picked: each arrives as a [`Event::FilePicked`] of its own.
        /// From API 0.2.
        #[serde(default, skip_serializing_if = "is_false")]
        multiple: bool,
        /// Where the copies go. From API 0.2.
        #[serde(default, skip_serializing_if = "PickInto::is_tmp")]
        into: PickInto,
    },
    OpenPanel {
        panel: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },
    /// Show a library file in the gallery (by its id).
    SelectModel {
        id: String,
    },
    /// Render again after this long (an animation, polling).
    RepaintAfter {
        ms: u32,
    },
    /// An effect of a newer API than this Meshgrove's: it is skipped, the others are done.
    #[serde(other)]
    Unknown,
}

/// Where a file the user picked is copied to ([`Effect::OpenFileDialog`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PickInto {
    /// The panel's `/tmp`, as `/tmp/<name>`: gone when the panel closes, and jobs do not see it
    /// (each has a `/tmp` of its own).
    #[default]
    Tmp,
    /// The plugin's `/data`, as `/data/picked/<name>`: it stays, and the panel's jobs read it
    /// too. A file of that name with other content is kept; the new one gets a number.
    Data,
}

impl PickInto {
    fn is_tmp(&self) -> bool {
        *self == PickInto::Tmp
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobSpec {
    /// The panel's name for it, in [`Event::JobProgress`] and [`Event::JobFinished`].
    pub id: String,
    /// What `run-job` gets as `kind`.
    pub kind: String,
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

// ---- Shorthands ----

impl Node {
    pub fn column(children: Vec<Node>) -> Node {
        Node::Column { children }
    }

    pub fn row(children: Vec<Node>) -> Node {
        Node::Row { children }
    }

    pub fn label(text: impl Into<String>) -> Node {
        Node::Label { text: text.into(), style: TextStyle::Body }
    }

    pub fn heading(text: impl Into<String>) -> Node {
        Node::Label { text: text.into(), style: TextStyle::Heading }
    }

    pub fn button(id: impl Into<String>, label: impl Into<String>) -> Node {
        Node::Button { id: id.into(), label: label.into(), primary: false, enabled: true }
    }

    /// A length in millimetres.
    pub fn mm(id: impl Into<String>, value: f64, min: f64, max: f64) -> Node {
        Node::NumberInput {
            id: id.into(),
            value,
            min: Some(min),
            max: Some(max),
            step: None,
            unit: Unit::Mm,
            decimals: Some(1),
            enabled: true,
        }
    }

    pub fn text_input(id: impl Into<String>, value: impl Into<String>) -> Node {
        Node::TextInput { id: id.into(), value: value.into(), hint: None, multiline: false, enabled: true }
    }

    /// Every node in the tree, depth first (this one first).
    pub fn walk(&self) -> Vec<&Node> {
        let mut out = vec![self];
        let mut i = 0;
        while i < out.len() {
            let node = out[i];
            match node {
                Node::Column { children } | Node::Row { children } => out.extend(children),
                Node::Group { children, .. } | Node::Scroll { children, .. } => out.extend(children),
                Node::Tabs { tabs, .. } => out.extend(tabs.iter().flat_map(|t| &t.children)),
                Node::Split { first, second, .. } => out.extend([first.as_ref(), second.as_ref()]),
                Node::Form { rows, .. } => out.extend(rows.iter().map(|r| &r.node)),
                _ => {}
            }
            i += 1;
        }
        out
    }

    /// The input with this id, anywhere in the tree.
    pub fn find(&self, id: &str) -> Option<&Node> {
        self.walk().into_iter().find(|n| n.id() == Some(id))
    }

    /// The id of an input (or a host widget).
    pub fn id(&self) -> Option<&str> {
        match self {
            Node::Tabs { id, .. }
            | Node::Button { id, .. }
            | Node::TextInput { id, .. }
            | Node::NumberInput { id, .. }
            | Node::Slider { id, .. }
            | Node::Checkbox { id, .. }
            | Node::Select { id, .. }
            | Node::Radio { id, .. }
            | Node::Color { id, .. }
            | Node::Form { id, .. }
            | Node::Table { id, .. }
            | Node::MeshView { id, .. } => Some(id),
            Node::GridEditor(g) => Some(&g.id),
            Node::Canvas(c) => Some(&c.id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundles_outcome_reads_as_documented() {
        let applied =
            Event::BundleFinished { title: "Box".into(), outcome: BundleOutcome::Applied { steps: 56, skipped: 0 } };
        let json = serde_json::json!({"type": "bundle_finished", "title": "Box",
            "outcome": {"status": "applied", "steps": 56, "skipped": 0}});
        assert_eq!(serde_json::to_value(&applied).unwrap(), json);
        assert_eq!(serde_json::from_value::<Event>(json).unwrap(), applied);
        for (outcome, json) in [
            (BundleOutcome::Queued, serde_json::json!({"status": "queued"})),
            (BundleOutcome::Refused, serde_json::json!({"status": "refused"})),
            (
                BundleOutcome::Failed { error: "disk full".into() },
                serde_json::json!({"status": "failed", "error": "disk full"}),
            ),
        ] {
            assert_eq!(serde_json::to_value(&outcome).unwrap(), json);
            assert_eq!(serde_json::from_value::<BundleOutcome>(json).unwrap(), outcome);
        }
    }

    #[test]
    fn events_of_a_newer_api_are_read_as_unknown() {
        let event: Event = serde_json::from_str(r#"{"type": "from_the_future", "x": 1}"#).unwrap();
        assert_eq!(event, Event::Unknown);
    }
    use serde_json::json;

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(value: &T) {
        let text = serde_json::to_string(value).unwrap();
        assert_eq!(&serde_json::from_str::<T>(&text).unwrap(), value, "{text}");
    }

    fn sample() -> View {
        View::new(Node::column(vec![
            Node::heading("Box"),
            Node::Form {
                id: "size".into(),
                rows: vec![
                    FormRow { label: "Width".into(), node: Node::mm("width", 20.0, 1.0, 300.0) },
                    FormRow { label: "Name".into(), node: Node::text_input("name", "box") },
                ],
                submit: None,
            },
            Node::Split {
                vertical: false,
                ratio: 0.6,
                first: Box::new(Node::GridEditor(GridEditor {
                    id: "grid".into(),
                    cols: 5,
                    rows: 7,
                    pitch: 42.0,
                    snap_div: 1,
                    zones: vec![GridZone {
                        id: "left".into(),
                        x: -0.46,
                        y: 0.0,
                        w: 0.46,
                        h: 7.0,
                        label: Some("19.5".into()),
                        color: None,
                        accepts_items: true,
                    }],
                    items: vec![GridItem {
                        id: "b1".into(),
                        x: 0.0,
                        y: 0.0,
                        w: 2.0,
                        h: 1.0,
                        color: Some("#4a90d9".into()),
                        label: Some("2×1".into()),
                        badges: vec!["6u".into()],
                        actions: vec![Action { id: "split".into(), label: "Split".into() }],
                        locked: false,
                    }],
                    selected: vec![],
                    height: None,
                })),
                second: Box::new(Node::MeshView {
                    id: "view".into(),
                    key: "box".into(),
                    revision: 3,
                    height: Some(240.0),
                    bed: None,
                    frame: Some([[0.0, 0.0, 0.0], [84.0, 42.0, 21.0]]),
                }),
            },
            Node::Canvas(Canvas {
                id: "outline".into(),
                width: 100.0,
                height: 50.0,
                shapes: vec![
                    Shape::Polygon {
                        points: vec![[0.0, 0.0], [10.0, 0.0], [5.0, 8.0]],
                        fill: None,
                        stroke: Stroke::default(),
                    },
                    Shape::Text { x: 1.0, y: 2.0, text: "A".into(), size: None, color: None },
                ],
                interactive: true,
                background: None,
            }),
            Node::Table {
                id: "bom".into(),
                columns: vec![TableColumn { title: "Part".into(), width: None, numeric: false }],
                rows: vec![TableRow { id: "p1".into(), cells: vec!["bin".into()] }],
                selected: None,
                max_height: None,
            },
            Node::button("apply", "Apply"),
            Node::Separator,
        ]))
    }

    #[test]
    fn views_round_trip() {
        let view = sample();
        round_trip(&view);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["v"], 1);
        assert_eq!(json["root"]["children"][1]["rows"][0]["node"]["unit"], "mm");
        assert!(json["root"]["children"][6].get("enabled").is_none(), "defaults stay out of the JSON");
        assert_eq!(view.root.find("width").and_then(Node::id), Some("width"));
        assert!(matches!(view.root.find("grid"), Some(Node::GridEditor(_))));
    }

    #[test]
    fn a_mesh_view_reads_with_and_without_a_frame() {
        // What a plugin built against an older API sends: no frame.
        let old: Node =
            serde_json::from_value(json!({"type": "mesh_view", "id": "v", "key": "k", "revision": 1})).unwrap();
        assert!(matches!(old, Node::MeshView { frame: None, bed: None, .. }), "{old:?}");
        assert!(!serde_json::to_string(&old).unwrap().contains("frame"));
        let with: Node = serde_json::from_value(json!({"type": "mesh_view", "id": "v", "key": "k", "revision": 1,
            "frame": [[0.0, 0.0, 0.0], [250.0, 305.0, 100.0]]}))
        .unwrap();
        assert!(matches!(with, Node::MeshView { frame: Some([_, [250.0, 305.0, 100.0]]), .. }), "{with:?}");
    }

    #[test]
    fn unknown_widgets_become_placeholders() {
        let json = json!({"v": 2, "root": {"type": "column", "children": [
            {"type": "hologram", "id": "x", "depth": 3},
            {"type": "label", "text": "still here"}
        ]}});
        let view: View = serde_json::from_value(json).unwrap();
        assert_eq!(view.root.walk()[1], &Node::Unknown);
        assert_eq!(view.root.walk()[2], &Node::label("still here"));
    }

    #[test]
    fn events_and_effects_round_trip() {
        let events = [
            Event::Click { id: "apply".into() },
            Event::Changed { id: "width".into(), value: json!(30.5) },
            Event::Submit { id: "size".into(), values: json!({"width": 30.0}).as_object().unwrap().clone() },
            Event::TableSelect { id: "bom".into(), row: Some("p1".into()) },
            Event::Grid { id: "grid".into(), change: GridChange::Moved { item: "b1".into(), x: 1.0, y: 2.0 } },
            Event::Grid { id: "grid".into(), change: GridChange::Action { item: "b1".into(), action: "split".into() } },
            Event::Canvas { id: "outline".into(), change: CanvasChange::Drag { from: [0.0, 0.0], to: [1.0, 1.0] } },
            Event::JobProgress { job: "j1".into(), fraction: 0.5, message: "half".into() },
            Event::JobFinished { job: "j1".into(), result: None, error: Some("broke".into()) },
            Event::FilePicked { id: "photo".into(), name: "a.png".into(), path: "/tmp/picked/a.png".into() },
            Event::HostChanged { info: HostInfo { language: "de".into(), ..HostInfo::default() } },
            Event::ModelsOpened { ids: vec!["Eigene/box.stl".into(), "Eigene/lid.3mf".into()] },
        ];
        events.iter().for_each(round_trip);
        assert_eq!(serde_json::to_value(&events[0]).unwrap(), json!({"type": "click", "id": "apply"}));
        let effects = [
            Effect::Toast { text: "done".into(), level: Level::Success },
            Effect::StartJobs {
                jobs: vec![JobSpec { id: "j1".into(), kind: "box".into(), params: json!({"w": 1}), label: None }],
            },
            Effect::CancelJob { job: "j1".into() },
            Effect::ApplyBundle { bundle: Bundle { title: "Box".into(), ..Bundle::default() } },
            Effect::ProposeBundle { bundle: Bundle { title: "Box".into(), ..Bundle::default() } },
            Effect::SaveFile { path: "/out/box.stl".into(), name: "box.stl".into() },
            Effect::OpenFileDialog {
                id: "photo".into(),
                title: None,
                extensions: vec!["png".into()],
                multiple: true,
                into: PickInto::Data,
            },
            Effect::OpenPanel { panel: "box".into(), state: None },
            Effect::SelectModel { id: "Eigene/box.stl".into() },
            Effect::RepaintAfter { ms: 100 },
        ];
        effects.iter().for_each(round_trip);
        let toast: Effect = serde_json::from_str(r#"{"type":"toast","text":"hi"}"#).unwrap();
        assert_eq!(toast, Effect::Toast { text: "hi".into(), level: Level::Info });
        // As an API 0.1 plugin asks for a file: one, into /tmp.
        let dialog: Effect = serde_json::from_str(r#"{"type":"open_file_dialog","id":"photo"}"#).unwrap();
        let plain = Effect::OpenFileDialog {
            id: "photo".into(),
            title: None,
            extensions: vec![],
            multiple: false,
            into: PickInto::Tmp,
        };
        assert_eq!(dialog, plain);
        assert_eq!(serde_json::to_value(&plain).unwrap(), json!({"type": "open_file_dialog", "id": "photo"}));
        assert_eq!(serde_json::to_value(&effects[6]).unwrap()["into"], "data");
    }

    #[test]
    fn effects_of_a_newer_api_do_not_spoil_the_list() {
        let effects: Vec<Effect> =
            serde_json::from_str(r#"[{"type": "hologram", "depth": 3}, {"type": "toast", "text": "still here"}]"#)
                .unwrap();
        assert_eq!(effects, [Effect::Unknown, Effect::Toast { text: "still here".into(), level: Level::Info }]);
    }
}
