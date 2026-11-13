//! "Duplikate" view: groups of exact copies and of models with the same geometry, with a
//! suggestion which copy to keep (or the user's choice) and a way to move the others to the
//! trash: group by group, or every group ticked as fine at once.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use egui::{Color32, RichText, Sense, Ui, vec2};
use meshgrove_core::listing::{self, Entry};

use super::gallery::{Gallery, Thumb};
use crate::i18n;

pub struct GroupInfo {
    /// The group's files in the view's entry list.
    pub range: Range<usize>,
    pub exact: bool,
    /// Entry index of the copy we suggest keeping.
    pub keep: usize,
    pub wasted: u64,
    /// Why the group's extra copies cannot be trashed (read-only drive, archive member).
    pub blocked: Option<String>,
    /// The user is happy with the group's choice: it is cleaned up with the others.
    pub picked: bool,
    /// What the group is one of (`Library::duplicate_groups`), to ignore it by.
    pub key: String,
    /// The user keeps these copies as they are; shown only on request.
    pub ignored: bool,
    /// One of an archive's models with an extracted copy: settled with the archive's row above
    /// (all of an archive's models at once), and listed among the groups only on request.
    pub in_archive: bool,
}

/// What the view shows besides the groups, and how.
pub struct Shown<'a> {
    /// How many groups the user keeps as they are, and whether they are listed too.
    pub ignored: usize,
    pub show_ignored: bool,
    /// The archives with extracted copies, decided as a whole above the groups.
    pub archives: &'a [ArchiveInfo],
    /// The setting, which makes one of each archive's buttons the main one.
    pub prefer_packed: bool,
    /// The groups that belong to those archives are listed too.
    pub show_archive_groups: bool,
}

/// An archive whose models have extracted copies among the duplicates: one decision for all
/// of them, instead of a locked group per member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveInfo {
    pub archive: PathBuf,
    /// Models the index has in the archive.
    pub members: usize,
    /// Of those, how many have an exact copy outside it.
    pub covered: usize,
    /// The copies outside, to trash when the archive is kept.
    pub copies: Vec<PathBuf>,
    /// What the copies take, and what the archive takes.
    pub copies_size: u64,
    pub size: u64,
    /// Why the archive cannot be trashed (a read-only drive).
    pub blocked: Option<String>,
    /// The groups (by index) its models are in.
    pub groups: Vec<usize>,
}

impl ArchiveInfo {
    /// Every model of the archive is extracted: the archive can go without losing anything.
    pub fn complete(&self) -> bool {
        self.members > 0 && self.covered >= self.members
    }

    /// The folders the copies lie in, each once, as the archive's row names them.
    pub fn folders(&self) -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = Vec::new();
        for c in &self.copies {
            if let Some(p) = c.parent()
                && !folders.iter().any(|f| f == p)
            {
                folders.push(p.to_path_buf());
            }
        }
        folders
    }
}

/// The archives among the exact groups (ignored ones left out) that have copies outside them,
/// in the order of their first group. `members_of` says how many models an archive holds.
pub fn archives_of(entries: &[Entry], groups: &[GroupInfo], members_of: impl Fn(&Path) -> usize) -> Vec<ArchiveInfo> {
    let mut found: BTreeMap<PathBuf, (usize, ArchiveInfo)> = BTreeMap::new();
    let mut order = 0;
    for (gi, group) in groups.iter().enumerate().filter(|(_, g)| g.exact && !g.ignored) {
        let files: Vec<&Entry> = group.range.clone().map(|i| &entries[i]).collect();
        let outside: Vec<&Entry> =
            files.iter().copied().filter(|f| meshgrove_core::archive::split(&f.path).is_none()).collect();
        if outside.is_empty() {
            continue;
        }
        let mut seen: Vec<PathBuf> = Vec::new();
        for f in &files {
            let Some((archive, _)) = meshgrove_core::archive::split(&f.path) else { continue };
            if seen.contains(&archive) {
                continue; // the same model twice in one archive counts once
            }
            seen.push(archive.clone());
            let (_, info) = found.entry(archive.clone()).or_insert_with(|| {
                order += 1;
                let info = ArchiveInfo {
                    members: members_of(&archive),
                    archive,
                    covered: 0,
                    copies: Vec::new(),
                    copies_size: 0,
                    size: 0,
                    blocked: None,
                    groups: Vec::new(),
                };
                (order, info)
            });
            info.covered += 1;
            info.groups.push(gi);
            for o in &outside {
                if !info.copies.contains(&o.path) {
                    info.copies.push(o.path.clone());
                    info.copies_size += o.size;
                }
            }
        }
    }
    let mut archives: Vec<(usize, ArchiveInfo)> = found.into_values().collect();
    archives.sort_by_key(|(order, _)| *order);
    archives.into_iter().map(|(_, a)| a).collect()
}

pub enum DupAction {
    Select(usize),
    Open(usize),
    /// Trash every file of the group except the one kept.
    TrashOthers(usize),
    /// Keep this entry of its group instead.
    Keep(usize),
    /// Tick or untick a group (its choice is fine).
    Pick(usize, bool),
    /// Tick (true) or untick every group that can be cleaned up.
    PickAll(bool),
    /// Trash the extra copies of every ticked group.
    TrashPicked,
    /// Keep this group as it is (true): it is no longer listed, counted or badged; or watch it
    /// again.
    Ignore(usize, bool),
    /// List the ignored groups too, or hide them again.
    ShowIgnored(bool),
    /// Keep the archive (by its index): its extracted copies go to the trash.
    TrashArchiveCopies(usize),
    /// Keep the extracted copies: the archive goes to the trash.
    TrashArchive(usize),
    /// Keep every archive: all their extracted copies go to the trash.
    TrashAllArchiveCopies,
    /// Keep the extracted copies everywhere: every archive that is extracted completely goes.
    TrashAllArchives,
    /// List the groups that belong to the archives too, or fold them away again.
    ShowArchiveGroups(bool),
}

/// The copy worth keeping: outside an archive (or, with `prefer_packed`, inside one), then the
/// oldest, then the shortest path.
pub fn suggest_keep(files: &[Entry], prefer_packed: bool) -> usize {
    (0..files.len())
        .min_by_key(|&i| {
            let f = &files[i];
            let packed = meshgrove_core::archive::split(&f.path).is_some();
            (packed != prefer_packed, f.modified, f.path.as_os_str().len())
        })
        .unwrap_or(0)
}

pub fn show(
    ui: &mut Ui,
    entries: &[Entry],
    groups: &[GroupInfo],
    view: &Shown,
    gallery: &Gallery,
) -> Option<DupAction> {
    let Shown { ignored, show_ignored, archives, prefer_packed, show_archive_groups } = *view;
    // An archive's groups are settled with its row; among the groups they would be one locked
    // card per model.
    let folded = |g: &GroupInfo| g.in_archive && !show_archive_groups;
    let mut action = None;
    if groups.is_empty() && ignored == 0 {
        ui.centered_and_justified(|ui| ui.weak(t!("dupes.none")));
        return None;
    }
    // The summary and the cleanup of all ticked groups stay on top while the groups scroll.
    let watched: Vec<&GroupInfo> = groups.iter().filter(|g| !g.ignored).collect();
    let wasted: u64 = watched.iter().map(|g| g.wasted).sum();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(t!("dupes.summary", groups = watched.len(), size = i18n::file_size(wasted))).strong());
        if ignored > 0 {
            ui.weak(t!("dupes.ignored_count", count = ignored));
            let label = if show_ignored { t!("dupes.hide_ignored") } else { t!("dupes.show_ignored") };
            if ui.small_button(label).clicked() {
                action = Some(DupAction::ShowIgnored(!show_ignored));
            }
        }
    });
    ui.weak(t!("dupes.explain"));
    ui.add_space(4.0);
    let open: Vec<&GroupInfo> = groups.iter().filter(|g| g.blocked.is_none() && !g.ignored && !folded(g)).collect();
    let picked: Vec<&GroupInfo> = open.iter().copied().filter(|g| g.picked).collect();
    ui.horizontal(|ui| {
        let mut all = !open.is_empty() && picked.len() == open.len();
        let some = !picked.is_empty() && !all;
        let check = egui::Checkbox::new(&mut all, t!("dupes.pick_all")).indeterminate(some);
        if ui.add_enabled(!open.is_empty(), check).changed() {
            action = Some(DupAction::PickAll(all));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let size = i18n::file_size(picked.iter().map(|g| g.wasted).sum());
            let label = match picked.len() {
                0 => t!("dupes.trash_picked_none"),
                n => t!("dupes.trash_picked", groups = n, size = size),
            };
            let button = ui
                .add_enabled(!picked.is_empty(), egui::Button::new(RichText::new(label).strong()))
                .on_disabled_hover_text(t!("dupes.trash_picked_hint"));
            if button.clicked() {
                action = Some(DupAction::TrashPicked);
            }
        });
    });
    ui.add_space(6.0);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        if !archives.is_empty() {
            ui.label(RichText::new(t!("dupes.archives_title")).strong());
            ui.weak(t!("dupes.archives_explain"));
            if let Some(act) = all_archives(ui, archives, prefer_packed) {
                action = Some(act);
            }
            for (ai, a) in archives.iter().enumerate() {
                if let Some(act) = archive_row(ui, a, prefer_packed, ai) {
                    action = Some(act);
                }
            }
            let theirs = groups.iter().filter(|g| g.in_archive).count();
            ui.horizontal_wrapped(|ui| {
                ui.weak(t!("dupes.archive_groups", count = theirs));
                let label =
                    if show_archive_groups { t!("dupes.hide_archive_groups") } else { t!("dupes.show_archive_groups") };
                if ui.small_button(label).clicked() {
                    action = Some(DupAction::ShowArchiveGroups(!show_archive_groups));
                }
            });
            ui.add_space(10.0);
        }
        for (gi, group) in groups.iter().enumerate().filter(|(_, g)| !folded(g)) {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    let mut on = group.picked;
                    let check = egui::Checkbox::without_text(&mut on);
                    let response = ui
                        .add_enabled(group.blocked.is_none() && !group.ignored, check)
                        .on_hover_text(t!("dupes.pick_group"))
                        .on_disabled_hover_text(group.blocked.clone().unwrap_or_default());
                    if response.changed() {
                        action = Some(DupAction::Pick(gi, on));
                    }
                    let title = if group.exact { t!("dupes.exact") } else { t!("dupes.geometry") };
                    ui.label(RichText::new(title).strong());
                    ui.weak(t!("dupes.group_info", count = group.range.len(), size = i18n::file_size(group.wasted)));
                    if group.ignored {
                        ui.label(RichText::new(t!("dupes.ignored")).weak().italics());
                    }
                    // The buttons stacked at the right edge, so they never run into the text.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                            if group.ignored {
                                if ui.button(t!("dupes.unignore")).on_hover_text(t!("dupes.unignore_hint")).clicked() {
                                    action = Some(DupAction::Ignore(gi, false));
                                }
                                return;
                            }
                            ui.horizontal(|ui| {
                                if let Some(reason) = &group.blocked {
                                    ui.label("🔒").on_hover_text(reason);
                                }
                                let button = ui
                                    .add_enabled(group.blocked.is_none(), egui::Button::new(t!("dupes.trash_others")))
                                    .on_disabled_hover_text(group.blocked.clone().unwrap_or_default());
                                if button.clicked() {
                                    action = Some(DupAction::TrashOthers(gi));
                                }
                            });
                            if ui.button(t!("dupes.ignore")).on_hover_text(t!("dupes.ignore_hint")).clicked() {
                                action = Some(DupAction::Ignore(gi, true));
                            }
                        });
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    for i in group.range.clone() {
                        let keep = format!("★ {}", t!("dupes.keep"));
                        let badge = (i == group.keep).then_some((keep.as_str(), KEEP_GREEN));
                        let (response, click) = tile(
                            ui,
                            &entries[i],
                            gallery.thumb(i),
                            (badge, false),
                            gallery.selected == Some(i),
                            TILE_WIDTH,
                            true,
                        );
                        // The others offer themselves for keeping where the pointer is.
                        let mut on_badge = false;
                        if i != group.keep && response.hovered() {
                            let img = image_rect(response.rect);
                            let label = format!("☆ {}", t!("dupes.keep"));
                            let fill = ui.visuals().widgets.inactive.bg_fill;
                            let fg = ui.visuals().strong_text_color();
                            let rect = super::icons::corner_badge(ui.painter(), img, &label, fill, fg);
                            on_badge = response.hover_pos().is_some_and(|p| rect.expand(2.0).contains(p));
                            if on_badge {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                        }
                        match click {
                            Some(TileClick::Select) if on_badge => action = Some(DupAction::Keep(i)),
                            Some(TileClick::Select) => action = Some(DupAction::Select(i)),
                            Some(TileClick::Open) => action = Some(DupAction::Open(i)),
                            None => {}
                        }
                    }
                });
            });
            ui.add_space(6.0);
        }
    });
    action
}

/// Every archive settled at once, the way the setting prefers first: keep the archives (all
/// extracted copies go) or keep the copies (the archives that are extracted completely go).
fn all_archives(ui: &mut Ui, archives: &[ArchiveInfo], prefer_packed: bool) -> Option<DupAction> {
    let mut action = None;
    let mut copies: Vec<&std::path::PathBuf> = archives.iter().flat_map(|a| &a.copies).collect();
    copies.sort();
    copies.dedup();
    let copies_size: u64 = archives.iter().map(|a| a.copies_size).sum();
    let whole: Vec<&ArchiveInfo> = archives.iter().filter(|a| a.complete() && a.blocked.is_none()).collect();
    let whole_size: u64 = whole.iter().map(|a| a.size).sum();
    ui.horizontal_wrapped(|ui| {
        let strong = |text: std::borrow::Cow<str>, main: bool| {
            let text = RichText::new(text);
            egui::Button::new(if main { text.strong() } else { text })
        };
        let keep_archives = t!(
            "dupes.keep_all_archives",
            archives = archives.len(),
            count = copies.len(),
            size = i18n::file_size(copies_size)
        );
        if ui.add(strong(keep_archives, prefer_packed)).on_hover_text(t!("dupes.keep_archive_hint")).clicked() {
            action = Some(DupAction::TrashAllArchiveCopies);
        }
        let keep_copies = t!("dupes.keep_all_copies", count = whole.len(), size = i18n::file_size(whole_size));
        let button = ui
            .add_enabled(!whole.is_empty(), strong(keep_copies, !prefer_packed))
            .on_disabled_hover_text(t!("dupes.keep_all_copies_none"));
        if button.on_hover_text(t!("dupes.keep_all_copies_hint")).clicked() {
            action = Some(DupAction::TrashAllArchives);
        }
    });
    ui.add_space(4.0);
    action
}

/// One archive with extracted copies: what it holds, where the copies are, and the two ways to
/// settle it. The way the setting prefers is the main button.
fn archive_row(ui: &mut Ui, a: &ArchiveInfo, prefer_packed: bool, ai: usize) -> Option<DupAction> {
    let mut action = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("📦 {}", listing::file_name(&a.archive))).strong());
            ui.add(
                egui::Label::new(
                    RichText::new(a.archive.parent().map(|p| p.display().to_string()).unwrap_or_default()).weak(),
                )
                .truncate(),
            );
        });
        let folders = a.folders();
        let mut names: Vec<String> = folders.iter().take(3).map(|f| f.display().to_string()).collect();
        if folders.len() > 3 {
            names.push(t!("dupes.more_folders", count = folders.len() - 3).into_owned());
        }
        ui.weak(t!("dupes.archive_info", members = a.members, covered = a.covered, folders = names.join(", ")));
        ui.horizontal_wrapped(|ui| {
            let keep_archive = t!("dupes.keep_archive", count = a.copies.len(), size = i18n::file_size(a.copies_size));
            let keep_copies = t!("dupes.keep_copies", size = i18n::file_size(a.size));
            let main = |text: std::borrow::Cow<str>, main: bool| {
                let text = RichText::new(text);
                egui::Button::new(if main { text.strong() } else { text })
            };
            if ui.add(main(keep_archive, prefer_packed)).on_hover_text(t!("dupes.keep_archive_hint")).clicked() {
                action = Some(DupAction::TrashArchiveCopies(ai));
            }
            let can_go = a.complete() && a.blocked.is_none();
            let why = match &a.blocked {
                Some(reason) if !reason.is_empty() => reason.clone(),
                _ => t!("dupes.keep_copies_blocked", missing = a.members.saturating_sub(a.covered)).into_owned(),
            };
            let button = ui.add_enabled(can_go, main(keep_copies, !prefer_packed)).on_disabled_hover_text(why);
            if button.on_hover_text(t!("dupes.keep_copies_hint")).clicked() {
                action = Some(DupAction::TrashArchive(ai));
            }
        });
    });
    ui.add_space(6.0);
    action
}

const KEEP_GREEN: Color32 = Color32::from_rgb(90, 170, 90);

pub(super) enum TileClick {
    Select,
    Open,
}

/// Width of the tiles in the duplicates and inbox views.
pub(super) const TILE_WIDTH: f32 = 170.0;

/// A file of a group, `width` wide: picture, name, folder, size and date; `badge` (text,
/// colour) in the top corner, `warn` a printability warning at the bottom. The response is for
/// context menus.
/// Where a tile of `rect` draws its picture.
fn image_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(rect.width() - 12.0, rect.width() - 12.0))
}

pub(super) fn tile(
    ui: &mut Ui,
    entry: &Entry,
    thumb: Option<&Thumb>,
    (badge, warn): (Option<(&str, Color32)>, bool),
    selected: bool,
    width: f32,
    full_path: bool,
) -> (egui::Response, Option<TileClick>) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, width + 64.0), Sense::click());
    let visuals = ui.visuals();
    let bg = if selected {
        visuals.selection.bg_fill.linear_multiply(0.55)
    } else if response.hovered() {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, bg);
    let img = image_rect(rect);
    if let Some(Thumb::Ready(texture, _)) = thumb {
        let size = texture.size_vec2();
        let scale = (img.width() / size.x).min(img.height() / size.y);
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(texture.id(), egui::Rect::from_center_size(img.center(), size * scale), uv, Color32::WHITE);
    }
    if let Some((label, color)) = badge {
        super::icons::corner_badge(&painter, img, label, color, Color32::BLACK);
    }
    if warn {
        super::icons::bottom_right_badge(
            &painter,
            img,
            "⚠",
            visuals.warn_fg_color.linear_multiply(0.9),
            Color32::BLACK,
        );
    }
    let text = visuals.text_color();
    let weak = visuals.weak_text_color();
    let mut y = img.bottom() + 4.0;
    for (line, color, size) in [
        (listing::file_name(&entry.path), text, 12.5),
        // Copies are told apart by their whole path; elsewhere the folder name does.
        match entry.path.parent() {
            Some(p) if full_path => (p.display().to_string(), weak, 11.0),
            Some(p) => (format!("📁 {}", listing::file_name(p)), weak, 11.0),
            None => (String::new(), weak, 11.0),
        },
        (
            format!("{} · {}", i18n::file_size(entry.size), entry.modified.map(i18n::datetime).unwrap_or_default()),
            weak,
            11.0,
        ),
    ] {
        let mut job = egui::text::LayoutJob::simple_singleline(line, egui::FontId::proportional(size), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(width - 10.0);
        let galley = painter.layout_job(job);
        let h = galley.size().y;
        painter.galley(egui::pos2(rect.min.x + 5.0, y), galley, color);
        y += h + 1.0;
    }
    let response = response.on_hover_text_at_pointer(entry.path.display().to_string());
    let click = if response.double_clicked() {
        Some(TileClick::Open)
    } else if response.clicked() {
        Some(TileClick::Select)
    } else {
        None
    };
    (response, click)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshgrove_core::formats::Kind;
    use std::time::{Duration, SystemTime};

    /// A library on disk: `archive::split` tells members from files by what is there. The
    /// archives are files (their contents do not matter here); the folders exist.
    fn library() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        for folder in ["packs", "castle", "old", "a", "b"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        for zip in ["castle.zip", "other.zip", "more.zip"] {
            std::fs::write(root.join("packs").join(zip), b"PK").unwrap();
        }
        (dir, root)
    }

    fn entry(root: &Path, path: &str, size: u64, age: u64) -> Entry {
        let path = root.join(path);
        Entry {
            name: listing::file_name(&path),
            path,
            kind: Kind::Stl,
            size,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - age)),
        }
    }

    fn group(range: Range<usize>, exact: bool, ignored: bool) -> GroupInfo {
        GroupInfo {
            range,
            exact,
            keep: 0,
            wasted: 0,
            blocked: None,
            picked: false,
            key: String::new(),
            ignored,
            in_archive: false,
        }
    }

    #[test]
    fn the_setting_says_which_copy_to_keep() {
        let (_dir, root) = library();
        // The packed copy is older; the extracted one has the shorter path.
        let packed = entry(&root, "packs/castle.zip/towers/tower.stl", 10, 100);
        let plain = entry(&root, "castle/tower.stl", 10, 10);
        let files = [packed, plain];
        assert_eq!(suggest_keep(&files, false), 1, "outside the archive");
        assert_eq!(suggest_keep(&files, true), 0, "inside the archive");
        let two_plain = [entry(&root, "a/tower.stl", 10, 10), entry(&root, "b/tower.stl", 10, 50)];
        assert_eq!(suggest_keep(&two_plain, true), 1, "no packed copy: the oldest");
    }

    #[test]
    fn archives_are_settled_as_a_whole() {
        let (_dir, root) = library();
        // castle.zip has 3 models in the index; two are extracted (one of them twice), one is not.
        let entries = vec![
            entry(&root, "packs/castle.zip/tower.stl", 10, 1),
            entry(&root, "castle/tower.stl", 10, 1),
            entry(&root, "old/tower.stl", 10, 1),
            entry(&root, "packs/castle.zip/wall.stl", 20, 1),
            entry(&root, "castle/wall.stl", 20, 1),
            entry(&root, "packs/castle.zip/gate.stl", 5, 1), // same geometry, not a copy
            entry(&root, "castle/gate.3mf", 7, 1),
            entry(&root, "packs/other.zip/x.stl", 1, 1), // packed twice, never extracted
            entry(&root, "packs/more.zip/x.stl", 1, 1),
        ];
        let groups = vec![
            group(0..3, true, false),
            group(3..5, true, false),
            group(5..7, false, false),
            group(7..9, true, false),
        ];
        let members = |a: &Path| if a.ends_with("castle.zip") { 3 } else { 1 };
        let archives = archives_of(&entries, &groups, members);
        assert_eq!(archives.len(), 1, "only archives with extracted copies");
        let castle = &archives[0];
        assert_eq!(castle.archive, root.join("packs").join("castle.zip"));
        assert_eq!((castle.members, castle.covered), (3, 2));
        assert!(!castle.complete(), "the gate is not extracted");
        assert_eq!(castle.groups, [0, 1], "the groups its row settles");
        assert_eq!(castle.copies.len(), 3);
        assert_eq!(castle.copies_size, 40);
        assert_eq!(castle.folders(), [root.join("castle"), root.join("old")]);

        // Ignored groups do not count; with every model extracted the archive can go.
        let groups = vec![group(0..3, true, false), group(3..5, true, false)];
        let all = archives_of(&entries, &groups, |_| 2);
        assert!(all[0].complete());
        let groups = vec![group(0..3, true, true), group(3..5, true, false)];
        assert_eq!(archives_of(&entries, &groups, |_| 2)[0].covered, 1);
    }
}
