//! What the update check shows: a pill in the title bar (or the toolbar, where the desktop draws
//! the title bar), the one question before anything is sent, and the window of a newer version.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use egui::{Rect, Sense, Ui, pos2, vec2};
use solidcraft_theme::Tokens;
use solidcraft_ui_kit as sc;

use super::kind::InstallKind;
use super::net::Progress;
use super::release::{self, SUMS};
use super::{Event, Job, Updater, kind, state};
use crate::ui::dialogs::say;

/// The kit's sign for a download into the tray.
const ICON: &str = "import_tray";

/// What the pill offers.
#[derive(Debug, Clone, PartialEq)]
enum Pill {
    /// The question has not been answered.
    Ask,
    /// This newer version is out.
    Update(String),
}

impl Pill {
    /// Its labels, longest first; an empty one is the sign alone.
    fn labels(&self) -> Vec<String> {
        match self {
            Pill::Ask => vec![t!("update.ask_pill").into(), t!("update.ask_pill_short").into()],
            Pill::Update(version) => {
                vec![t!("update.pill", version = version).into(), t!("update.pill_short").into(), String::new()]
            }
        }
    }

    fn accent(&self) -> bool {
        matches!(self, Pill::Update(_))
    }

    fn tooltip(&self) -> String {
        match self {
            Pill::Ask => t!("update.ask_tooltip").into(),
            Pill::Update(version) => t!("update.pill_tooltip", version = version).into(),
        }
    }
}

/// How wide a pill with `label` is, and how tall.
fn pill_size(ui: &Ui, label: &str, accent: bool) -> egui::Vec2 {
    let t = Tokens::get();
    let text = if label.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(label.to_owned(), egui::FontId::proportional(t.font_label), egui::Color32::WHITE)
            .size()
            .x
    };
    let sign = if accent { t.space_16 + if label.is_empty() { 0.0 } else { t.space_4 } } else { 0.0 };
    vec2(text + sign + 2.0 * t.space_8, t.control_height - t.space_4)
}

/// Draws a pill in `rect`. `on_bar`: on the title bar's colours.
fn paint_pill(ui: &mut Ui, rect: Rect, pill: &Pill, label: &str, on_bar: bool) -> egui::Response {
    let t = Tokens::get();
    let response = ui.interact(rect, egui::Id::unique(("update_pill", pill.accent())), Sense::click());
    if ui.is_rect_visible(rect) {
        let (fill, ink) = if pill.accent() {
            (t.form_primary, crate::ui::settings_kit::ink_on_accent())
        } else if on_bar {
            (t.app_bar_hover_button, t.app_bar_text)
        } else {
            (t.accent_soft, t.form_ink)
        };
        let fill = if response.hovered() { fill.gamma_multiply(0.85) } else { fill };
        let painter = ui.painter();
        painter.rect_filled(rect, rect.height() / 2.0, fill);
        if !pill.accent() {
            let stroke = egui::Stroke::new(1.0, ink.gamma_multiply(0.35));
            painter.rect_stroke(rect, rect.height() / 2.0, stroke, egui::StrokeKind::Inside);
        }
        let mut x = rect.left() + t.space_8;
        if pill.accent() {
            let sign = Rect::from_min_size(pos2(x, rect.center().y - t.space_16 / 2.0), vec2(t.space_16, t.space_16));
            sc::icons::paint_ink(painter, sign, ICON, ink);
            x = sign.right() + t.space_4;
        }
        if !label.is_empty() {
            let font = egui::FontId::proportional(t.font_label);
            painter.text(pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, label, font, ink);
        }
    }
    let tooltip = pill.tooltip();
    response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip)
}

impl Updater {
    fn pill(&self, asked: Option<bool>) -> Option<Pill> {
        if !self.available {
            return None;
        }
        if asked.is_none() {
            return Some(Pill::Ask);
        }
        let version = self.newer.as_ref()?.version.to_string();
        let shown = !self.memo.hides(&version, state::now()) || self.window || self.job.is_some();
        shown.then_some(Pill::Update(version))
    }

    fn clicked(&mut self, pill: &Pill) {
        match pill {
            Pill::Ask => self.asking = true,
            Pill::Update(_) => self.window = !self.window,
        }
    }

    /// The pill in the title bar, at the right end of `room` (what the title leaves free). In a
    /// narrow window it says less, then shows its sign alone, then nothing.
    pub fn title_pill(&mut self, ui: &mut Ui, room: Rect, asked: Option<bool>) {
        let Some(pill) = self.pill(asked) else { return };
        let gap = Tokens::get().space_8;
        let fitting =
            pill.labels().into_iter().find(|label| pill_size(ui, label, pill.accent()).x + gap <= room.width());
        let Some(label) = fitting else { return };
        let size = pill_size(ui, &label, pill.accent());
        let rect = Rect::from_min_size(pos2(room.right() - gap - size.x, room.center().y - size.y / 2.0), size);
        if paint_pill(ui, rect, &pill, &label, true).clicked() {
            self.clicked(&pill);
        }
    }

    /// The pill in the toolbar (a right-to-left row), where the desktop draws the title bar.
    pub fn toolbar_pill(&mut self, ui: &mut Ui, asked: Option<bool>) {
        let Some(pill) = self.pill(asked) else { return };
        let label = pill.labels().into_iter().next().unwrap_or_default();
        let (rect, _) = ui.allocate_exact_size(pill_size(ui, &label, pill.accent()), Sense::hover());
        if paint_pill(ui, rect, &pill, &label, false).clicked() {
            self.clicked(&pill);
        }
    }

    /// The question and the update window, when open. What the app has to do comes back.
    pub fn windows(&mut self, ctx: &egui::Context) -> Vec<Event> {
        let mut events = Vec::new();
        if !self.available {
            return events;
        }
        if self.asking
            && let Some(event) = self.question(ctx)
        {
            events.push(event);
        }
        if matches!(self.job, Some(Job::Installer)) {
            self.job = None;
            self.window = false;
            events.push(Event::Quit);
        }
        if self.window
            && let Some(event) = self.update_window(ctx)
        {
            events.push(event);
        }
        events
    }

    /// The one question before anything is sent: what the check does, yes or no. Closing it
    /// without an answer leaves the pill to ask again.
    fn question(&mut self, ctx: &egui::Context) -> Option<Event> {
        let (title, yes, no) = (t!("update.ask_title"), t!("update.ask_yes"), t!("update.ask_no"));
        let mut card = sc::Confirm::new(egui::Id::unique("update_question"), ICON, &title)
            .tone(sc::Tone::Info)
            .actions(&yes, &no, None);
        card.window = card.window.cards(1.5);
        let (closed, action) = card.show(ctx, true, |ui| {
            ui.style_mut().wrap_mode = None;
            say(ui, &t!("update.ask_text"));
            ui.add_space(Tokens::get().space_8);
            say(ui, &t!("update.ask_where"));
        });
        let answer = match action {
            sc::Action::Ok => Some(true),
            sc::Action::Cancel => Some(false),
            _ => None,
        };
        if answer.is_some() || closed {
            self.asking = false;
        }
        answer.map(Event::Answered)
    }

    fn update_window(&mut self, ctx: &egui::Context) -> Option<Event> {
        let Some(latest) = &self.newer else {
            self.window = false;
            return None;
        };
        let kind = kind();
        let version = latest.version.to_string();
        let asset = kind.asset_name(&latest.version).and_then(|name| latest.asset(&name)).cloned();
        let one_click = kind.one_click() && asset.is_some() && latest.asset(SUMS).is_some();
        let browser = asset.as_ref().map(|a| a.url.clone()).unwrap_or_else(|| latest.page.clone());
        let (notes, page, command) =
            (release::readable_notes(&latest.notes), latest.page.clone(), kind.command(&latest.version));

        enum Phase {
            Offer(Option<String>),
            Downloading(Arc<Progress>),
            Ready,
            Opened,
        }
        let phase = match &self.job {
            None => Phase::Offer(None),
            Some(Job::Failed(e)) => Phase::Offer(Some(e.clone())),
            Some(Job::Downloading { progress, .. }) => Phase::Downloading(progress.clone()),
            Some(Job::Ready(_)) => Phase::Ready,
            Some(Job::Opened) => Phase::Opened,
            Some(Job::Installer) => return None,
        };
        let (ok, cancel, apply): (String, String, Option<String>) = match &phase {
            Phase::Offer(failed) => {
                let main = if one_click && failed.is_none() {
                    main_label(kind)
                } else if asset.is_none() {
                    t!("update.open_page").into()
                } else {
                    t!("update.download").into()
                };
                (main, t!("update.later").into(), Some(t!("update.skip").into()))
            }
            Phase::Downloading(_) => (String::new(), t!("update.cancel").into(), None),
            Phase::Ready => (t!("update.restart").into(), t!("update.restart_later").into(), None),
            Phase::Opened => (t!("update.ok").into(), String::new(), None),
        };
        let title = t!("update.window_title", version = version);
        let mut window = sc::Window::new(egui::Id::unique("update_window"), ICON, &title)
            .actions(&ok, &cancel, apply.as_deref())
            .cards(2.0)
            .fields();
        if matches!(phase, Phase::Downloading(_)) {
            window = window.required();
        }
        let md = &mut self.notes;
        let (closed, action) = window.show(ctx, true, |ui| {
            ui.style_mut().wrap_mode = None;
            let t = Tokens::get();
            match &phase {
                Phase::Offer(failed) => {
                    say(ui, &t!("update.versions", installed = release::current().to_string(), new = version));
                    if let Some(e) = failed {
                        ui.add_space(t.space_8);
                        ui.colored_label(t.form_error, t!("update.failed", message = e));
                    }
                    ui.add_space(t.space_8);
                    say(ui, &hint(kind));
                    if let Some(command) = &command {
                        ui.add_space(t.space_4);
                        ui.horizontal(|ui| {
                            ui.code(command);
                            if sc::button(ui, t!("update.copy"), sc::Button::Small).clicked() {
                                ui.ctx().copy_text(command.clone());
                            }
                        });
                    }
                    if !notes.is_empty() {
                        ui.add_space(t.space_12);
                        ui.label(egui::RichText::new(t!("update.notes")).strong());
                        egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                            egui_commonmark::CommonMarkViewer::new().show(ui, md, &notes);
                        });
                    }
                    if !page.is_empty() {
                        ui.add_space(t.space_4);
                        ui.hyperlink_to(t!("update.page"), &page);
                    }
                }
                Phase::Downloading(progress) => {
                    let mb = |bytes: u64| format!("{:.1}", bytes as f64 / 1_048_576.0);
                    let (done, total) = (progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed));
                    say(ui, &t!("update.downloading", done = mb(done), total = mb(total)));
                    ui.add_space(t.space_8);
                    sc::progress(ui, "update_download", progress.fraction());
                }
                Phase::Ready => say(ui, &t!("update.ready", version = version)),
                Phase::Opened => say(ui, &t!("update.opened", place = mac_place(kind))),
            }
        });
        let now = state::now();
        match (&phase, action) {
            (Phase::Offer(failed), sc::Action::Ok) => {
                if one_click && failed.is_none() {
                    self.install(ctx);
                } else {
                    ctx.open_url(egui::OpenUrl::new_tab(browser));
                }
            }
            (Phase::Offer(_), sc::Action::Cancel) => {
                self.memo.later(now);
                self.save_memo();
                self.window = false;
            }
            (Phase::Offer(_), sc::Action::Apply) => {
                self.memo.skip(&version);
                self.save_memo();
                self.window = false;
            }
            (Phase::Downloading(progress), sc::Action::Cancel) => progress.cancel.store(true, Ordering::Relaxed),
            (Phase::Ready, sc::Action::Ok) => {
                if let Some(Job::Ready(program)) = &self.job {
                    self.relaunch = Some(program.clone());
                }
                self.window = false;
                return Some(Event::Quit);
            }
            (Phase::Ready | Phase::Opened, sc::Action::Ok | sc::Action::Cancel) => self.window = false,
            _ if closed => self.window = false,
            _ => {}
        }
        None
    }
}

/// The main button where Meshgrove updates itself.
fn main_label(kind: &InstallKind) -> String {
    match kind {
        InstallKind::WindowsSetup { .. } => t!("update.install_setup").into(),
        InstallKind::MacApp(_) => t!("update.install_mac").into(),
        _ => t!("update.install_now").into(),
    }
}

/// What the main button does for this installation, and what stays the same.
fn hint(kind: &InstallKind) -> String {
    match kind {
        InstallKind::AppImage(_) => t!("update.hint_appimage").into(),
        InstallKind::WindowsSetup { .. } => t!("update.hint_setup").into(),
        InstallKind::WindowsPortable(_) => t!("update.hint_portable").into(),
        InstallKind::MacApp(_) => t!("update.hint_mac", place = mac_place(kind)).into(),
        InstallKind::Deb(_) | InstallKind::Rpm(_) => t!("update.hint_package").into(),
        InstallKind::Source => t!("update.hint_source").into(),
        InstallKind::Unpacked | InstallKind::MacOther | InstallKind::Dev => t!("update.hint_page").into(),
    }
}

/// Where the app bundle is, for the new one to go to the same place.
fn mac_place(kind: &InstallKind) -> String {
    match kind {
        InstallKind::MacApp(bundle) => bundle.display().to_string(),
        _ => "/Applications/Meshgrove.app".to_owned(),
    }
}
