//! The list view: the gallery's entries as a table with a small preview per row. Selection,
//! keyboard, context menu and thumbnails are the grid's; columns can be resized, hidden (right-
//! click a column title) and sorted (click it).

use std::borrow::Cow;
use std::collections::HashSet;

use egui::{Align, Color32, Layout, Rect, RichText, Sense, Ui, pos2, vec2};
use egui_extras::{Column as Width, TableBuilder};
use meshgrove_core::config::{ListColumns, SortKey};
use meshgrove_core::listing::{self, Entry};
use meshgrove_core::printer::Fit;

use super::{Action, FAVORITE_GOLD, Gallery, GalleryOutput, MenuContext, PRINTED_GREEN, Thumb, entry_menu, fit_inside};
use crate::i18n;
use crate::thumbs::ThumbService;
use crate::ui::{icons, tags};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Preview,
    Name,
    /// The containing folder: only in search results, lists and "include subfolders".
    Folder,
    Type,
    Size,
    Dimensions,
    Modified,
    Tags,
    Favorite,
    Print,
}

impl Column {
    pub const ALL: [Column; 10] = [
        Column::Preview,
        Column::Name,
        Column::Folder,
        Column::Type,
        Column::Size,
        Column::Dimensions,
        Column::Modified,
        Column::Tags,
        Column::Favorite,
        Column::Print,
    ];

    /// Name in the settings file.
    pub fn id(self) -> &'static str {
        match self {
            Column::Preview => "preview",
            Column::Name => "name",
            Column::Folder => "folder",
            Column::Type => "type",
            Column::Size => "size",
            Column::Dimensions => "dimensions",
            Column::Modified => "modified",
            Column::Tags => "tags",
            Column::Favorite => "favorite",
            Column::Print => "print",
        }
    }

    pub fn title(self) -> Cow<'static, str> {
        match self {
            Column::Preview => t!("list.col_preview"),
            Column::Name => t!("list.col_name"),
            Column::Folder => t!("list.col_folder"),
            Column::Type => t!("list.col_type"),
            Column::Size => t!("list.col_size"),
            Column::Dimensions => t!("list.col_dimensions"),
            Column::Modified => t!("list.col_modified"),
            Column::Tags => t!("list.col_tags"),
            Column::Favorite => t!("list.col_favorite"),
            Column::Print => t!("list.col_print"),
        }
    }

    /// The column's head: the title, or a symbol where the column is narrow.
    fn heading(self) -> Cow<'static, str> {
        match self {
            Column::Favorite => "★".into(),
            _ => self.title(),
        }
    }

    /// What clicking the title sorts by.
    fn sort_key(self) -> Option<SortKey> {
        match self {
            Column::Name => Some(SortKey::Name),
            Column::Type => Some(SortKey::Type),
            Column::Size => Some(SortKey::Size),
            Column::Modified => Some(SortKey::Modified),
            _ => None,
        }
    }

    fn default_width(self) -> f32 {
        match self {
            Column::Preview => 0.0, // follows the row height
            Column::Name => 240.0,
            Column::Folder => 140.0,
            Column::Type => 64.0,
            Column::Size => 80.0,
            Column::Dimensions => 145.0,
            Column::Modified => 120.0,
            Column::Tags => 150.0,
            Column::Favorite => 24.0,
            Column::Print => 56.0,
        }
    }
}

/// The app's settings the list view shows and changes.
pub struct ListSettings<'a> {
    /// Hidden columns and dragged widths (written back).
    pub columns: &'a mut ListColumns,
    /// The current order, shown by an arrow in its column's title.
    pub sort: SortKey,
    pub descending: bool,
}

/// How rows look: the row height follows the tile size (the zoom slider and Ctrl + wheel).
pub fn row_height(tile: f32) -> f32 {
    (tile / 4.0).clamp(24.0, 64.0)
}

impl Gallery {
    /// The entries as a table. Clicking a column title answers with [`Action::SortBy`]; hidden
    /// columns and dragged widths go to `list.columns`.
    pub fn show_list(
        &mut self,
        ui: &mut Ui,
        entries: &[Entry],
        tile: f32,
        service: &ThumbService,
        menu: &MenuContext,
        list: ListSettings<'_>,
    ) -> GalleryOutput {
        let ListSettings { columns, sort, descending } = list;
        let row_h = row_height(tile);
        let shown_cols: Vec<Column> = Column::ALL
            .into_iter()
            .filter(|c| !columns.hidden.iter().any(|h| h == c.id()))
            .filter(|c| *c != Column::Folder || self.show_folders)
            .collect();
        let mut action = self.keyboard(ui, 1);
        let viewport = ui.available_rect_before_wrap();

        // Widths a new table starts with: as set before, or the defaults with the name taking
        // what is left of the pane.
        let spacing = ui.spacing().item_spacing.x;
        let set = |c: &Column| columns.widths.get(c.id()).copied();
        let others: f32 = shown_cols
            .iter()
            .filter(|c| **c != Column::Name)
            .map(|c| if *c == Column::Preview { row_h } else { set(c).unwrap_or(c.default_width()) } + spacing)
            .sum();
        let initial: Vec<f32> = shown_cols
            .iter()
            .map(|c| match c {
                Column::Preview => row_h,
                Column::Name => set(c).unwrap_or((ui.available_width() - others - spacing).max(160.0)),
                _ => set(c).unwrap_or(c.default_width()),
            })
            .collect();
        let mut visible = HashSet::new();
        let mut widths = Vec::new();
        let mut hide: Option<(Column, bool)> = None;
        // Wider than the pane when many columns are shown: then it scrolls sideways too.
        egui::ScrollArea::horizontal().id_salt("file_list_h").auto_shrink(false).show(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .id_salt(("file_list", shown_cols.len()))
                .striped(true)
                .resizable(true)
                .sense(Sense::click())
                .auto_shrink(false)
                .max_scroll_height(f32::INFINITY)
                .cell_layout(Layout::left_to_right(Align::Center));
            for (c, &w) in shown_cols.iter().zip(&initial) {
                table = table.column(match c {
                    Column::Preview => Width::exact(row_h),
                    _ => Width::initial(w).at_least(20.0).clip(true),
                });
            }
            if std::mem::take(&mut self.scroll_to_selected)
                && let Some(pos) = self.selected.and_then(|s| self.shown.iter().position(|&i| i == s))
            {
                table = table.scroll_to_row(pos, None);
            }

            table
                .header(22.0, |mut header| {
                    for &c in &shown_cols {
                        let (_, response) = header.col(|ui| {
                            if c == Column::Preview {
                                return;
                            }
                            let arrow = match c.sort_key() {
                                Some(key) if key == sort => {
                                    if descending {
                                        " ▼"
                                    } else {
                                        " ▲"
                                    }
                                }
                                _ => "",
                            };
                            let title = RichText::new(format!("{}{arrow}", c.heading())).strong();
                            match c.sort_key() {
                                Some(key) => {
                                    let button = ui.add(egui::Button::new(title).frame(false));
                                    if button.clicked() {
                                        action = Some(Action::SortBy(key));
                                    }
                                    column_menu(&button, columns, self.show_folders, &mut hide);
                                }
                                None => {
                                    ui.add(egui::Label::new(title).truncate());
                                }
                            }
                        });
                        column_menu(&response, columns, self.show_folders, &mut hide);
                    }
                })
                .body(|body| {
                    widths = body.widths().to_vec();
                    body.rows(row_h, self.shown.len(), |mut row| {
                        let entry = self.shown[row.index()];
                        let e = &entries[entry];
                        visible.extend(self.keys.get(entry).cloned().flatten());
                        row.set_selected(self.is_marked(entry));
                        for &c in &shown_cols {
                            row.col(|ui| self.cell(ui, c, e, entry, row_h));
                        }
                        let response = row.response();
                        if response.clicked() {
                            let modifiers = ui_modifiers(&response);
                            self.click(entry, modifiers);
                        }
                        if response.secondary_clicked() && !self.is_marked(entry) {
                            self.select(Some(entry));
                            self.scroll_to_selected = false;
                        }
                        if response.double_clicked() {
                            action = Some(Action::Open(entry));
                        }
                        entry_menu(&response, entry, e, self.show_folders, menu, &mut action);
                        response.on_hover_text_at_pointer(self.tooltip(e, entry));
                    });
                });
        });

        if let Some((c, show)) = hide {
            columns.hidden.retain(|h| h != c.id());
            if !show {
                columns.hidden.push(c.id().to_owned());
            }
        }
        // Remember widths the user dragged (the preview column follows the row height).
        for ((c, w), start) in shown_cols.iter().zip(widths).zip(initial) {
            if *c != Column::Preview && (w - start).abs() > 0.5 {
                columns.widths.insert(c.id().to_owned(), w);
            }
        }
        if visible != self.visible {
            service.set_visible(|k| visible.contains(k));
            self.visible = visible;
        }
        GalleryOutput { action, hovered: None, viewport }
    }

    fn cell(&self, ui: &mut Ui, column: Column, e: &Entry, index: usize, row_h: f32) {
        let weak = ui.visuals().weak_text_color();
        match column {
            Column::Preview => self.mini_preview(ui, e, index, row_h),
            Column::Name => {
                ui.add(egui::Label::new(listing::file_name(&e.path)).truncate().selectable(false));
            }
            Column::Folder => {
                let folder = e.path.parent().map(listing::file_name).unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(folder).color(weak)).truncate().selectable(false));
            }
            Column::Type => {
                let label = icons::extension_label(&e.name);
                let text = if e.is_dir() || label.is_empty() { crate::ui::details::kind_name(e.kind) } else { label };
                ui.add(egui::Label::new(RichText::new(text).color(weak)).truncate().selectable(false));
            }
            Column::Size => {
                if !e.is_dir() {
                    right_aligned(ui, &i18n::file_size(e.size));
                }
            }
            Column::Dimensions => {
                let Some(Thumb::Ready(_, meta)) = self.thumb(index) else { return };
                let Some(size) = meta.size_mm else { return };
                let text = i18n::dimensions(glam::Vec3::from(size));
                let color = match self.bed_fit(e, index) {
                    Some((Fit::Rotated(_), _)) => ui.visuals().warn_fg_color,
                    Some((Fit::TooBig(_), _)) => ui.visuals().error_fg_color,
                    _ => ui.visuals().text_color(),
                };
                ui.add(egui::Label::new(RichText::new(text).color(color)).truncate().selectable(false));
            }
            Column::Modified => {
                if let Some(modified) = e.modified {
                    ui.add(egui::Label::new(RichText::new(i18n::datetime(modified)).color(weak)).truncate());
                }
            }
            Column::Tags => {
                let Some(marks) = self.marks.get(&e.path).filter(|m| !m.tags.is_empty()) else { return };
                let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
                let painter = ui.painter_at(rect);
                let mut x = rect.left();
                let y = rect.center().y - 8.0;
                for tag in &marks.tags {
                    if x > rect.right() - 12.0 {
                        break;
                    }
                    let pill = if self.tag_dots {
                        tags::paint_dot(&painter, pos2(x + 12.0, y + 2.0), self.tag_style.colors(tag).0);
                        Rect::from_min_size(pos2(x, y), vec2(12.0, 12.0))
                    } else {
                        tags::paint_pill_from_left(&painter, pos2(x, y), tag, self.tag_style.colors(tag), 120.0)
                    };
                    x = pill.right() + 4.0;
                }
            }
            Column::Favorite => {
                if self.marks.get(&e.path).is_some_and(|m| m.favorite) {
                    ui.label(RichText::new("★").color(FAVORITE_GOLD));
                }
            }
            Column::Print => {
                if let Some(&(wanted, done)) = self.list_counts.get(&e.path) {
                    let color = if done >= wanted { PRINTED_GREEN } else { ui.visuals().text_color() };
                    ui.label(RichText::new(format!("{done}/{wanted}")).color(color));
                } else if let Some(m) = self.marks.get(&e.path).filter(|m| m.printed > 0 || m.last_failed) {
                    if m.last_failed {
                        ui.label(RichText::new("✗").color(ui.visuals().error_fg_color));
                    } else {
                        ui.label(RichText::new(format!("✓{}", m.printed)).color(PRINTED_GREEN));
                    }
                }
            }
        }
    }

    fn mini_preview(&self, ui: &mut Ui, e: &Entry, index: usize, row_h: f32) {
        let side = row_h - 4.0;
        let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
        let painter = ui.painter_at(rect);
        let paper = ui.visuals().widgets.inactive.bg_fill;
        let ink = ui.visuals().text_color();
        match self.thumb(index) {
            _ if e.is_dir() => icons::folder(&painter, rect),
            Some(Thumb::Ready(texture, _)) => {
                let fit = fit_inside(texture.size_vec2(), rect);
                painter.image(texture.id(), fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            Some(Thumb::Failed(_)) => icons::file(&painter, rect, "", paper, ui.visuals().warn_fg_color),
            _ => icons::file(&painter, rect, "", paper, ink),
        }
    }
}

/// Right-click on a column title: show or hide columns (the name always stays).
fn column_menu(
    response: &egui::Response,
    columns: &ListColumns,
    show_folders: bool,
    change: &mut Option<(Column, bool)>,
) {
    response.context_menu(|ui| {
        ui.label(RichText::new(t!("list.columns")).weak());
        for c in Column::ALL.into_iter().filter(|c| *c != Column::Name) {
            let mut on = !columns.hidden.iter().any(|h| h == c.id());
            let mut label = c.title().into_owned();
            if c == Column::Folder && !show_folders {
                label = format!("{label} {}", t!("list.folder_hint"));
            }
            if ui.checkbox(&mut on, label).changed() {
                *change = Some((c, on));
            }
        }
    });
}

fn right_aligned(ui: &mut Ui, text: &str) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add(egui::Label::new(text).selectable(false));
    });
}

fn ui_modifiers(response: &egui::Response) -> egui::Modifiers {
    response.ctx.input(|i| i.modifiers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_have_distinct_names_and_rows_follow_the_zoom() {
        let ids: HashSet<&str> = Column::ALL.iter().map(|c| c.id()).collect();
        assert_eq!(ids.len(), Column::ALL.len());
        assert!(Column::ALL.iter().all(|c| !c.title().is_empty()));
        assert_eq!((row_height(96.0), row_height(160.0), row_height(384.0)), (24.0, 40.0, 64.0));
    }
}
