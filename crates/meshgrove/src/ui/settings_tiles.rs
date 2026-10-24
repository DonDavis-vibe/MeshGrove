//! Settings → Tiles: a sample tile carrying every mark, whose marks are dragged into the corners
//! of its picture (or out of sight), and how tags, filaments, the check and the name look.

use egui::{
    Color32, CursorIcon, Id, Key, LayerId, Order, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui,
    pos2, vec2,
};
use meshgrove_core::config::{
    CheckStyle, Config, FilamentStyle, TILE_SIZE, TILE_TAG_COUNT, TileBadge, TileLayout, TileSlot, TileTags,
};

use super::gallery;
use super::settings::SettingsChange;
use super::tags::{self, Group, TagStyle};
use super::tile_overlay::{self, Check, Overlays, PrintMark};

/// The sample tile's picture: one of the author's designs as the thumbnailer renders it.
const SAMPLE: &[u8] = include_bytes!("../../assets/tile-sample.png");

/// Room around the sample tile, in the gallery's colour.
const MARGIN: f32 = 12.0;

#[derive(Default)]
pub struct TileSettings {
    sample: Option<TextureHandle>,
    /// The mark being dragged.
    dragging: Option<TileBadge>,
}

fn badge_name(badge: TileBadge) -> String {
    match badge {
        TileBadge::FileType => t!("settings.tile_file_type"),
        TileBadge::Filaments => t!("settings.tile_filaments"),
        TileBadge::Duplicates => t!("settings.tile_duplicates"),
        TileBadge::Tags => t!("settings.tile_tags"),
        TileBadge::Check => t!("settings.tile_check"),
        TileBadge::Favorite => t!("settings.tile_favorite"),
        TileBadge::PrintState => t!("settings.tile_print_state"),
    }
    .into_owned()
}

fn slot_name(slot: TileSlot) -> String {
    match slot {
        TileSlot::TopLeft => t!("settings.tile_top_left"),
        TileSlot::TopRight => t!("settings.tile_top_right"),
        TileSlot::BottomLeft => t!("settings.tile_bottom_left"),
        TileSlot::BottomRight => t!("settings.tile_bottom_right"),
        TileSlot::Hidden => t!("settings.tile_hide"),
    }
    .into_owned()
}

/// The four shortest suggested themes (long names would be cut on the tile), so the sample shows
/// a few colours and a "+N"; the other groups fill in when the themes were emptied.
fn sample_tags(style: &TagStyle) -> Vec<String> {
    let mut picked = Vec::new();
    for group in [Group::Themes, Group::Properties, Group::State] {
        let mut names = tags::suggested(group, style);
        names.sort_by_key(|t| t.chars().count());
        picked.extend(names.into_iter().take(4 - picked.len()));
        if picked.len() == 4 {
            break;
        }
    }
    picked.sort_by_key(|t| t.to_lowercase());
    picked
}

/// The quarter of the picture that stands for a corner.
fn zone(img: Rect, slot: TileSlot) -> Rect {
    let c = img.center();
    match slot {
        TileSlot::TopLeft => Rect::from_min_max(img.min, c),
        TileSlot::TopRight => Rect::from_min_max(pos2(c.x, img.min.y), pos2(img.max.x, c.y)),
        TileSlot::BottomLeft => Rect::from_min_max(pos2(img.min.x, c.y), pos2(c.x, img.max.y)),
        TileSlot::BottomRight | TileSlot::Hidden => Rect::from_min_max(c, img.max),
    }
}

/// Where a mark dragged to `pointer` lands: the corner of the picture under it and the place in
/// that corner's list (counted without the dragged mark: before the first mark drawn past the
/// pointer, else last), or the hidden marks over the tray. `None` anywhere else: the drag is
/// dropped and nothing moves.
pub(super) fn drop_target(
    pointer: Pos2,
    img: Rect,
    tray: Rect,
    layout: &TileLayout,
    placed: &[(TileBadge, Rect)],
    dragged: TileBadge,
) -> Option<(TileSlot, usize)> {
    if tray.contains(pointer) {
        return Some((TileSlot::Hidden, usize::MAX));
    }
    let slot = TileSlot::CORNERS.into_iter().find(|&s| img.contains(pointer) && zone(img, s).contains(pointer))?;
    let rect_of = |badge: TileBadge| placed.iter().find(|(b, _)| *b == badge).map(|(_, r)| *r);
    let past = |r: Rect| match slot {
        TileSlot::TopLeft | TileSlot::TopRight => pointer.y < r.center().y,
        TileSlot::BottomLeft => pointer.x < r.center().x,
        _ => pointer.x > r.center().x,
    };
    let index = layout
        .slot(slot)
        .iter()
        .filter(|&&b| b != dragged)
        .position(|&b| rect_of(b).is_some_and(past))
        .unwrap_or(usize::MAX);
    Some((slot, index))
}

impl TileSettings {
    /// The sample tile, the hidden marks below it, and the tile size.
    pub fn preview(&mut self, ui: &mut Ui, config: &Config, style: &TagStyle, changes: &mut Vec<SettingsChange>) {
        ui.label(RichText::new(t!("settings.tiles_preview")).strong());
        let layout = &config.tile_layout;
        let mut size = config.tile_size;
        ui.horizontal(|ui| {
            ui.label(t!("gallery.tile_size"));
            if ui.add(egui::Slider::new(&mut size, TILE_SIZE).show_value(false)).changed() {
                changes.push(SettingsChange::TileSize(size));
            }
        });

        // The tile on a strip in the gallery's colour.
        let width = size.min(ui.available_width() - 2.0 * MARGIN).max(*TILE_SIZE.start());
        let height = gallery::tile_height(ui.ctx(), width, layout.name_lines, false);
        let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), height + 2.0 * MARGIN), Sense::hover());
        let visuals = ui.visuals().clone();
        let painter = ui.painter_at(strip);
        painter.rect(strip, 6.0, visuals.panel_fill, visuals.widgets.noninteractive.bg_stroke, StrokeKind::Inside);
        let tile = Rect::from_center_size(strip.center(), vec2(width, height));
        let img = gallery::image_rect(tile);
        let texture = self.sample.get_or_insert_with(|| {
            let image = image::load_from_memory(SAMPLE).expect("bundled sample is a valid PNG").into_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
            ui.ctx().load_texture("tile-sample", image, egui::TextureOptions::LINEAR)
        });
        let fit = gallery::fit_inside(texture.size_vec2(), img);
        painter.image(texture.id(), fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let sample_tags = sample_tags(style);
        let overlays = Overlays {
            file_type: Some("3MF".to_owned()),
            filaments: vec![[214, 76, 66], [52, 168, 156], [236, 236, 230]],
            duplicates: Some(2),
            tags: &sample_tags,
            check: Some(Check::Rotated("90°".to_owned())),
            favorite: true,
            print: Some(PrintMark::Printed(2)),
        };
        let look = tile_overlay::Look {
            layout,
            tag_dots: config.tile_tags == TileTags::Dots && config.colored_tags,
            tag_count: usize::from(config.tile_tag_count),
            tag_style: style,
        };
        let placed = tile_overlay::paint(&painter, img, &overlays, &look, &visuals);
        let name = gallery::tile_name(t!("settings.tiles_sample_name").into_owned(), layout.name_extension);
        let top = pos2(tile.center().x, img.bottom() + 4.0);
        gallery::paint_name(&painter, top, width - 4.0, name, layout.name_lines, visuals.text_color());

        // The hidden marks as chips in a tray: dragged out to show them, dragged in to hide.
        ui.add_space(6.0);
        ui.label(t!("settings.tiles_hidden"));
        let (tray, chips) = self.tray(ui, layout);
        ui.weak(t!("settings.tiles_hint"));

        let mut targets = placed.clone();
        targets.extend(chips);
        for &(badge, rect) in &targets {
            let response = ui.interact(rect, Id::unique(("tile-mark", badge)), Sense::click_and_drag());
            if response.drag_started() {
                self.dragging = Some(badge);
            }
            if self.dragging.is_none() && response.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
                let stroke = Stroke::new(1.5, visuals.selection.stroke.color);
                ui.painter().rect_stroke(rect.expand(2.0), 4.0, stroke, StrokeKind::Outside);
            }
            let current = layout.slot_of(badge);
            response.context_menu(|ui| {
                for slot in TileSlot::CORNERS.into_iter().chain([TileSlot::Hidden]) {
                    if ui.add_enabled(slot != current, egui::Button::new(slot_name(slot))).clicked() {
                        let mut moved = layout.clone();
                        moved.place(badge, slot, usize::MAX);
                        changes.push(SettingsChange::TileLayout(moved));
                        ui.close();
                    }
                }
            });
            if self.dragging.is_none() {
                response.on_hover_text(badge_name(badge));
            }
        }

        let Some(badge) = self.dragging else { return };
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            self.dragging = None;
            return;
        }
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let target = pointer.and_then(|p| drop_target(p, img, tray, layout, &placed, badge));
        let highlight = visuals.selection.bg_fill.linear_multiply(0.35);
        let faint = Stroke::new(1.0, visuals.selection.stroke.color.linear_multiply(0.5));
        for slot in TileSlot::CORNERS {
            let rect = zone(img, slot).shrink(1.0);
            if target.is_some_and(|(s, _)| s == slot) {
                painter.rect_filled(rect, 4.0, highlight);
            } else {
                painter.rect_stroke(rect, 4.0, faint, StrokeKind::Inside);
            }
        }
        if target.is_some_and(|(s, _)| s == TileSlot::Hidden) {
            ui.painter().rect_filled(tray, 6.0, highlight);
        }
        if let Some((slot, index)) = target.filter(|(s, _)| *s != TileSlot::Hidden) {
            let (from, to) = insertion_mark(img, layout, &placed, badge, slot, index);
            painter.line_segment([from, to], Stroke::new(2.0, visuals.selection.stroke.color));
        }
        if let Some(p) = pointer {
            // The dragged mark's name follows the pointer, above everything.
            let ghost = ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::unique("tile-mark-ghost")));
            let colors = (visuals.selection.bg_fill, visuals.selection.stroke.color);
            tags::paint_pill_from_left(&ghost, p + vec2(14.0, 10.0), &badge_name(badge), colors, 240.0);
        }
        if ui.input(|i| !i.pointer.primary_down()) {
            if let Some((slot, index)) = target {
                let mut moved = layout.clone();
                moved.place(badge, slot, index);
                if moved != *layout {
                    changes.push(SettingsChange::TileLayout(moved));
                }
            }
            self.dragging = None;
        }
    }

    /// Draws the tray of hidden marks; returns its rectangle and where each chip went.
    fn tray(&self, ui: &mut Ui, layout: &TileLayout) -> (Rect, Vec<(TileBadge, Rect)>) {
        const CHIP_ROW: f32 = 22.0;
        let width = ui.available_width();
        let painter = ui.painter().clone();
        // Chips flow left to right, wrapping at the edge.
        let sizes: Vec<_> =
            layout.hidden.iter().map(|&b| (b, tags::pill_size(&painter, &badge_name(b), width - 12.0))).collect();
        let mut rows = 1;
        let mut x = 0.0;
        for (_, size) in &sizes {
            if x > 0.0 && x + size.x > width - 12.0 {
                rows += 1;
                x = 0.0;
            }
            x += size.x + 6.0;
        }
        let (tray, _) = ui.allocate_exact_size(vec2(width, rows as f32 * CHIP_ROW + 8.0), Sense::hover());
        let visuals = ui.visuals();
        painter.rect_stroke(tray, 6.0, visuals.widgets.noninteractive.bg_stroke, StrokeKind::Inside);
        if sizes.is_empty() {
            let weak = visuals.weak_text_color();
            let font = egui::TextStyle::Small.resolve(ui.style());
            painter.text(tray.center(), egui::Align2::CENTER_CENTER, t!("settings.tiles_hidden_empty"), font, weak);
            return (tray, Vec::new());
        }
        let colors = (visuals.widgets.inactive.bg_fill, visuals.text_color());
        let mut chips = Vec::new();
        let mut at = tray.min + vec2(6.0, 4.0 + 2.5);
        for (badge, size) in sizes {
            if at.x > tray.min.x + 6.0 && at.x + size.x > tray.max.x - 6.0 {
                at = pos2(tray.min.x + 6.0, at.y + CHIP_ROW);
            }
            let rect = tags::paint_pill_from_left(&painter, at, &badge_name(badge), colors, width - 12.0);
            chips.push((badge, rect));
            at.x = rect.max.x + 6.0;
        }
        (tray, chips)
    }

    /// How tags, filaments, the check and the name look; and back to the tiles as they came.
    pub fn look(&self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        ui.label(RichText::new(t!("settings.tiles_look")).strong());
        let mut layout = config.tile_layout.clone();
        let (mut count, mut style) = (config.tile_tag_count, config.tile_tags);
        egui::Grid::new("tile_look").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            ui.label(t!("settings.tile_tags"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut style, TileTags::Names, t!("settings.tags_names"));
                ui.add_enabled_ui(config.colored_tags, |ui| {
                    ui.radio_value(&mut style, TileTags::Dots, t!("settings.tags_dots"))
                        .on_disabled_hover_text(t!("settings.tags_dots_need_colors"));
                });
            });
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                ui.label(t!("settings.tags_count"));
                ui.add(egui::Slider::new(&mut count, TILE_TAG_COUNT));
            });
            ui.end_row();
            ui.label(t!("settings.tile_filaments"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut layout.filaments, FilamentStyle::Dots, t!("settings.tiles_filament_dots"));
                ui.radio_value(&mut layout.filaments, FilamentStyle::Stripe, t!("settings.tiles_filament_stripe"));
            });
            ui.end_row();
            ui.label(t!("settings.tile_check"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut layout.check, CheckStyle::Text, t!("settings.tiles_check_text"));
                ui.radio_value(&mut layout.check, CheckStyle::Symbol, t!("settings.tiles_check_symbol"));
            });
            ui.end_row();
            ui.label(t!("settings.tiles_name"));
            ui.horizontal(|ui| {
                for (lines, label) in [
                    (0, t!("settings.tiles_name_none")),
                    (1, t!("settings.tiles_name_one")),
                    (2, t!("settings.tiles_name_two")),
                ] {
                    ui.radio_value(&mut layout.name_lines, lines, label);
                }
            });
            ui.end_row();
            ui.label("");
            ui.add_enabled(
                layout.name_lines > 0,
                egui::Checkbox::new(&mut layout.name_extension, t!("settings.tiles_extension")),
            );
            ui.end_row();
        });
        ui.weak(t!("settings.tiles_look_hint"));
        if ui.button(t!("settings.suggest_reset")).clicked() {
            let standard = Config::default();
            layout = standard.tile_layout;
            (count, style) = (standard.tile_tag_count, standard.tile_tags);
        }
        if layout != config.tile_layout {
            changes.push(SettingsChange::TileLayout(layout));
        }
        if (count, style) != (config.tile_tag_count, config.tile_tags) {
            changes.push(SettingsChange::TileTagLook { count, style });
        }
    }
}

/// The line showing where a dragged mark would go: before the `index`-th mark of `slot`
/// (counted without the dragged one) or after its last drawn mark.
fn insertion_mark(
    img: Rect,
    layout: &TileLayout,
    placed: &[(TileBadge, Rect)],
    dragged: TileBadge,
    slot: TileSlot,
    index: usize,
) -> (Pos2, Pos2) {
    let drawn: Vec<(usize, Rect)> = layout
        .slot(slot)
        .iter()
        .filter(|&&b| b != dragged)
        .enumerate()
        .filter_map(|(i, b)| placed.iter().find(|(p, _)| p == b).map(|(_, r)| (i, *r)))
        .collect();
    let before = drawn.iter().find(|(i, _)| *i >= index).map(|(_, r)| *r);
    let last = drawn.last().map(|(_, r)| *r);
    let edge_x = if slot.is_left() { img.min.x + 4.0 } else { img.max.x - 4.0 };
    let span = if slot.is_left() { 36.0 } else { -36.0 };
    if slot.is_top() {
        let y = match (before, last) {
            (Some(r), _) => r.min.y - 1.0,
            (None, Some(r)) => r.max.y + 1.0,
            (None, None) => img.min.y + 3.0,
        };
        (pos2(edge_x, y), pos2(edge_x + span, y))
    } else {
        let x = match (before, last, slot.is_left()) {
            (Some(r), _, true) => r.min.x - 1.5,
            (Some(r), _, false) => r.max.x + 1.5,
            (None, Some(r), true) => r.max.x + 1.5,
            (None, Some(r), false) => r.min.x - 1.5,
            (None, None, _) => edge_x,
        };
        (pos2(x, img.max.y - 22.0), pos2(x, img.max.y - 4.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMG: Rect = Rect { min: Pos2::new(0.0, 0.0), max: Pos2::new(148.0, 148.0) };
    const TRAY: Rect = Rect { min: Pos2::new(0.0, 200.0), max: Pos2::new(300.0, 230.0) };

    /// The default marks where the gallery draws them on a 160 px tile.
    fn placed() -> Vec<(TileBadge, Rect)> {
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        vec![
            (TileBadge::Filaments, r(4.0, 4.0, 60.0, 19.0)),
            (TileBadge::Duplicates, r(122.0, 4.0, 144.0, 21.0)),
            (TileBadge::Tags, r(90.0, 24.0, 144.0, 76.0)),
            (TileBadge::FileType, r(4.0, 127.0, 34.0, 144.0)),
            (TileBadge::Check, r(104.0, 127.0, 144.0, 144.0)),
            (TileBadge::Favorite, r(84.0, 127.0, 101.0, 144.0)),
            (TileBadge::PrintState, r(58.0, 127.0, 81.0, 144.0)),
        ]
    }

    #[test]
    fn marks_land_in_the_corner_under_the_pointer_and_in_order() {
        let layout = TileLayout::default();
        let target = |x: f32, y: f32, badge| drop_target(pos2(x, y), IMG, TRAY, &layout, &placed(), badge);
        // Above the duplicate badge: first in the top right.
        assert_eq!(target(130.0, 6.0, TileBadge::Tags), Some((TileSlot::TopRight, 0)));
        // Below the tags: last.
        assert_eq!(target(130.0, 70.0, TileBadge::Favorite), Some((TileSlot::TopRight, usize::MAX)));
        // Bottom right counts from the corner leftwards: between check and favourite.
        assert_eq!(target(102.0, 140.0, TileBadge::FileType), Some((TileSlot::BottomRight, 1)));
        // Bottom left counts from the corner rightwards: before the type badge.
        assert_eq!(target(10.0, 140.0, TileBadge::Tags), Some((TileSlot::BottomLeft, 0)));
        // Its own corner is counted without it: past the check's middle it goes first.
        assert_eq!(target(140.0, 140.0, TileBadge::Favorite), Some((TileSlot::BottomRight, 0)));
        assert_eq!(target(120.0, 140.0, TileBadge::Favorite), Some((TileSlot::BottomRight, 1)));
        assert_eq!(target(40.0, 215.0, TileBadge::Check), Some((TileSlot::Hidden, usize::MAX)));
        assert_eq!(target(200.0, 100.0, TileBadge::Check), None, "outside: the drag is dropped");
        assert_eq!(target(74.0, 160.0, TileBadge::Check), None, "below the picture (the name)");
    }

    #[test]
    fn a_dropped_mark_moves_where_the_target_says() {
        let mut layout = TileLayout::default();
        let (slot, index) = drop_target(pos2(130.0, 6.0), IMG, TRAY, &layout, &placed(), TileBadge::Tags).unwrap();
        layout.place(TileBadge::Tags, slot, index);
        assert_eq!(layout.top_right, [TileBadge::Tags, TileBadge::Duplicates]);
        let (slot, index) =
            drop_target(pos2(10.0, 140.0), IMG, TRAY, &layout, &placed(), TileBadge::Filaments).unwrap();
        layout.place(TileBadge::Filaments, slot, index);
        assert_eq!(layout.bottom_left, [TileBadge::Filaments, TileBadge::FileType]);
        assert!(layout.top_left.is_empty());
    }
}
