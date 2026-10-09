//! The reader, in PdfCraft's chrome: tab strip, mode bar, tool rails, pasteboard, and the
//! translation card beside the paragraph it belongs to.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Align2, Color32, CornerRadius, Frame, Id, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder, Vec2, vec2,
};
use pdfcraft_render::{inspect, DocInfo, OutlineItem, RenderConfig};

use crate::icons;
use crate::paragraphs::Paragraph;
use crate::render::{PageSlot, RenderClient};
use crate::store::{Job, Stage, Store, Update, Worker};
use crate::theme::{self, ThemeKind, ThemePreference, Tokens};
use crate::typeface::{self, Typeface};
use crate::widgets;

const PAGE_GAP: f32 = 20.0;
const CARD_GAP: f32 = 10.0;
/// Inner padding of the translation card.
const CARD_PADDING: f32 = 16.0;
/// Padding inside the small chip, and the width its spinner takes before the label.
const CHIP_PADDING: f32 = 11.0;
const CHIP_SPINNER: f32 = 18.0;
/// PdfCraft's selection blue, laid over the text without hiding it.
fn hover_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(0x3A, 0x7B, 0xF0, 70)
}
const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "Deutsch"),
    ("fr", "Français"),
    ("es", "Español"),
    ("it", "Italiano"),
    ("pt", "Português"),
    ("nl", "Nederlands"),
    ("pl", "Polski"),
    ("ru", "Русский"),
    ("uk", "Українська"),
    ("tr", "Türkçe"),
];
const MODELS: &[&str] = &["qwen2.5:1.5b", "qwen2.5:3b"];

/// Zoom, language, model and theme, kept across restarts.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub zoom: f32,
    pub target: String,
    pub model: String,
    pub theme: ThemePreference,
}

impl Default for Settings {
    fn default() -> Self {
        Self { zoom: 1.25, target: "en".to_string(), model: MODELS[0].to_string(), theme: ThemePreference::Light }
    }
}

pub struct Doc {
    pub path: PathBuf,
    pub info: DocInfo,
    pub client: RenderClient,
    pub store: Arc<Mutex<Store>>,
    /// The family the document's body text is set in, for rendering translations in kind.
    pub face: Typeface,
}

/// One page's geometry for this frame, so overlays draw after the pages.
struct PageGeom {
    page: usize,
    band: Rect,
    page_rect: Rect,
    paragraphs: Vec<Rect>,
}

enum CardAction {
    None,
    Dismiss,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RightPanel {
    Outline,
    Translations,
    Log,
}

#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    Open,
    Close,
    ZoomIn,
    ZoomOut,
    Zoom100,
    FitWidth,
    Panel(RightPanel),
    Theme(ThemePreference),
    ClearTranslations,
    Shortcuts,
}

/// One row of the command palette.
#[derive(Clone, Copy)]
struct Command {
    label: &'static str,
    /// The shortcut or the group it belongs to, right-aligned.
    detail: &'static str,
    icon: &'static str,
    cmd: Cmd,
}

/// Substring first, then a subsequence fallback, as PdfCraft's palette does.
fn score(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay = hay.to_lowercase();
    if let Some(position) = hay.find(needle) {
        return Some(position);
    }
    let mut chars = hay.chars();
    needle.chars().all(|c| chars.any(|x| x == c)).then_some(100)
}

pub struct App {
    doc: Option<Doc>,
    settings: Settings,
    kind: ThemeKind,
    worker: Option<Worker>,
    /// Paragraph key → stage, for requests that are running (or failed).
    stages: HashMap<String, Stage>,
    /// The paragraph whose card is open (page, paragraph index).
    active: Option<(usize, usize)>,
    /// Paragraphs the reader closed; hovering them stays quiet until clicked again.
    dismissed: HashSet<(usize, usize)>,
    /// Where the card was last frame, so the pointer can move into it without closing.
    card_rect: Option<Rect>,
    card_rects: Vec<Rect>,
    /// Where the click-to-translate chip was last frame, so it stays while the pointer is on it.
    chip_rect: Option<Rect>,
    /// A PDF chosen in the file dialog, opened at the start of the next frame.
    pending_open: Option<PathBuf>,
    toast: Option<(String, f64)>,
    error: Option<String>,
    right: Option<RightPanel>,
    pan: bool,
    goto: Option<usize>,
    filter: String,
    palette: bool,
    palette_query: String,
    palette_focus: bool,
    shortcuts: bool,
    fit_width: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::install_fonts(&cc.egui_ctx);
        let settings = cc
            .storage
            .and_then(|storage| eframe::get_value::<Settings>(storage, eframe::APP_KEY))
            .unwrap_or_default();
        let kind = settings.theme.resolve(cc.egui_ctx.system_theme(), ThemeKind::Light);
        theme::apply(&cc.egui_ctx, kind);
        let mut app = Self {
            doc: None,
            settings,
            kind,
            worker: None,
            stages: HashMap::new(),
            active: None,
            dismissed: HashSet::new(),
            card_rect: None,
            card_rects: Vec::new(),
            chip_rect: None,
            toast: None,
            error: None,
            right: None,
            pan: false,
            goto: None,
            filter: String::new(),
            palette: false,
            palette_query: String::new(),
            palette_focus: false,
            shortcuts: false,
            fit_width: false,
            pending_open: None,
        };
        if let Some(path) = std::env::args().nth(1).map(PathBuf::from) {
            if path.is_file() {
                app.open(path, &cc.egui_ctx);
            }
        }
        if let Ok(page) = std::env::var("PDF_TRANSLATE_PAGE") {
            if let Ok(page) = page.parse() {
                app.goto = Some(page);
            }
        }
        app
    }

    fn target_name(&self) -> &'static str {
        LANGUAGES.iter().find(|(code, _)| *code == self.settings.target).map(|(_, name)| *name).unwrap_or("English")
    }

    fn toast(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), 0.0));
    }

    /// Pick a PDF and queue it. The document is opened at the start of the next frame, so a
    /// trigger from inside the central panel (the welcome button) cannot be overwritten when
    /// the taken-out document is put back.
    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
            self.pending_open = Some(path);
        }
    }

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.error = None;
        match std::fs::read(&path) {
            Ok(bytes) => {
                let bytes = Arc::new(bytes);
                match inspect(bytes.clone(), None) {
                    Ok(info) => {
                        let pages = info.pages.len();
                        let store = Arc::new(Mutex::new(Store::load(&path, &self.settings.target, &self.settings.model)));
                        let client = RenderClient::new(bytes, pages, RenderConfig::default(), ctx.clone());
                        self.worker = Some(Worker::spawn(&self.settings.model, store.clone(), ctx.clone()));
                        self.stages.clear();
                        self.active = None;
                        self.dismissed.clear();
                        self.card_rect = None;
                        self.goto = None;
                        self.fit_width = true;
                        self.toast(format!("{} — {pages} pages", info.title.clone().unwrap_or_else(|| file_name(&path))));
                        let face = typeface::of_document(&info.fonts);
                        self.doc = Some(Doc { path, info, client, store, face });
                    }
                    Err(error) => self.error = Some(error.to_string()),
                }
            }
            Err(error) => self.error = Some(format!("cannot read {}: {error}", path.display())),
        }
    }

    fn close_document(&mut self) {
        self.doc = None;
        self.worker = None;
        self.stages.clear();
        self.active = None;
        self.dismissed.clear();
        self.card_rect = None;
        self.goto = None;
    }

    fn respawn_worker(&mut self, ctx: &egui::Context) {
        self.stages.clear();
        if let Some(doc) = &self.doc {
            self.worker = Some(Worker::spawn(&self.settings.model, doc.store.clone(), ctx.clone()));
        }
    }

    fn poll_updates(&mut self) {
        let Some(worker) = &self.worker else { return };
        let mut updates = Vec::new();
        while let Some(update) = worker.try_recv() {
            updates.push(update);
        }
        for update in updates {
            match update {
                Update::Stage(key, stage) => {
                    if let Stage::Failed(error) = &stage {
                        self.toast(format!("translation failed: {error}"));
                    }
                    self.stages.insert(key, stage);
                }
                Update::Done { key } => {
                    self.stages.remove(&key);
                    self.toast("Translated a paragraph");
                }
            }
        }
    }

    /// Queue one paragraph for translation (unless it is cached or already running).
    fn request(&mut self, store: &Arc<Mutex<Store>>, key: String, text: String, page: usize) {
        let cached = store.lock().map(|store| store.get(&key).is_some()).unwrap_or(false);
        // A failed request may be retried; a queued or running one must not be duplicated.
        let in_flight = self.stages.get(&key).is_some_and(|stage| !matches!(stage, Stage::Failed(_)));
        if cached || in_flight {
            return;
        }
        let Some(worker) = &self.worker else { return };
        self.stages.insert(key.clone(), Stage::Queued);
        worker.submit(Job { key, text, target: self.settings.target.clone(), page });
    }

    fn zoom_by(&mut self, delta: f32) {
        self.settings.zoom = (self.settings.zoom + delta).clamp(0.25, 6.0);
    }

    fn run_command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Open => self.open_dialog(),
            Cmd::Close => self.close_document(),
            Cmd::ZoomIn => self.zoom_by(0.1),
            Cmd::ZoomOut => self.zoom_by(-0.1),
            Cmd::Zoom100 => self.settings.zoom = 1.0,
            Cmd::FitWidth => self.fit_width = true,
            Cmd::Panel(panel) => self.right = if self.right == Some(panel) { None } else { Some(panel) },
            Cmd::Theme(preference) => self.settings.theme = preference,
            Cmd::ClearTranslations => {
                if let Some(doc) = &self.doc {
                    if let Ok(mut store) = doc.store.lock() {
                        if let Err(error) = store.clear() {
                            self.error = Some(error);
                        }
                    }
                }
                self.stages.clear();
                self.toast("Translations cleared");
            }
            Cmd::Shortcuts => self.shortcuts = true,
        }
    }

    fn commands(&self) -> Vec<Command> {
        let mut commands = vec![
            Command { label: "Open a PDF", detail: "Ctrl+O", icon: "plus", cmd: Cmd::Open },
            Command { label: "Close the document", detail: "Ctrl+W", icon: "x", cmd: Cmd::Close },
            Command { label: "Zoom in", detail: "Ctrl++", icon: "zoom-in", cmd: Cmd::ZoomIn },
            Command { label: "Zoom out", detail: "Ctrl+−", icon: "zoom-out", cmd: Cmd::ZoomOut },
            Command { label: "Actual size (100%)", detail: "Ctrl+0", icon: "gauge", cmd: Cmd::Zoom100 },
            Command { label: "Fit to width", detail: "Ctrl+2", icon: "maximize", cmd: Cmd::FitWidth },
            Command { label: "Outline", detail: "Panel", icon: "list", cmd: Cmd::Panel(RightPanel::Outline) },
            Command { label: "Translations", detail: "Panel", icon: "languages", cmd: Cmd::Panel(RightPanel::Translations) },
            Command { label: "Log", detail: "Panel", icon: "square-terminal", cmd: Cmd::Panel(RightPanel::Log) },
            Command { label: "Light gray theme", detail: "Theme", icon: "sun", cmd: Cmd::Theme(ThemePreference::Light) },
            Command { label: "Dark gray theme", detail: "Theme", icon: "moon", cmd: Cmd::Theme(ThemePreference::Dark) },
            Command { label: "Use system theme", detail: "Theme", icon: "settings", cmd: Cmd::Theme(ThemePreference::System) },
            Command { label: "Keyboard shortcuts", detail: "Help", icon: "circle-help", cmd: Cmd::Shortcuts },
        ];
        if self.doc.is_some() {
            commands.push(Command { label: "Clear translations of this document", detail: "Document", icon: "trash-2", cmd: Cmd::ClearTranslations });
        }
        commands
    }

    // ---------------------------------------------------------------- chrome

    fn tab_strip(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let t = Tokens::get(ctx);
        egui::Panel::top("tab_strip")
            .exact_size(38.0)
            .frame(Frame::NONE.fill(t.titlebar).inner_margin(Margin { left: 8, right: 10, top: 0, bottom: 0 }))
            .show(ui, |ui| {
                let full = ui.max_rect();
                let drag = ui.interact(full, ui.id().with("title_drag"), Sense::click_and_drag());
                if drag.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if drag.double_clicked() {
                    let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if icons::button(ui, "house", 28.0, self.doc.is_none(), "Home").clicked() {
                        self.close_document();
                    }
                    let mut close = false;
                    if let Some(doc) = &self.doc {
                        let name = file_name(&doc.path);
                        close = document_tab(ui, &t, &name);
                    }
                    if close {
                        self.close_document();
                    }
                    if widgets::ghost_button(ui, "plus", "Open").on_hover_text("Open a PDF (⌘O)").clicked() {
                        self.open_dialog();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let icon = self.settings.theme.icon();
                        let tip = format!("Display theme: {}", self.settings.theme.label());
                        let response = icons::button(ui, icon, 28.0, false, &tip);
                        egui::Popup::menu(&response).show(|ui| {
                            for preference in [ThemePreference::System, ThemePreference::Light, ThemePreference::Dark] {
                                if ui.radio(self.settings.theme == preference, preference.label()).clicked() {
                                    self.settings.theme = preference;
                                    ui.close();
                                }
                            }
                        });
                        if icons::button(ui, "circle-help", 28.0, false, "Keyboard shortcuts").clicked() {
                            self.shortcuts = true;
                        }
                        if icons::button(ui, "info", 28.0, false, "About").clicked() {
                            self.toast("PDF Translate — reading with local-model translations");
                        }
                    });
                });
            });
    }

    fn mode_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let t = Tokens::get(ctx);
        egui::Panel::top("mode_bar")
            .exact_size(48.0)
            .frame(Frame::NONE.fill(t.chrome).inner_margin(Margin::symmetric(10, 0)).stroke(Stroke::new(1.0, t.divider)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    self.main_menu(ui);
                    widgets::vline(ui, 24.0, 6.0);
                    for (panel, label) in [(None, "Read"), (Some(RightPanel::Outline), "Outline"), (Some(RightPanel::Translations), "Translations"), (Some(RightPanel::Log), "Log")] {
                        if widgets::mode_tab(ui, label, self.right == panel).clicked() {
                            self.right = panel;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if widgets::search_box(ui, "Find a command", 190.0).clicked() {
                            self.palette = true;
                            self.palette_focus = true;
                        }
                        ui.add_space(6.0);
                        self.model_button(ui, ctx);
                        ui.add_space(2.0);
                        self.language_button(ui, ctx);
                        ui.add_space(8.0);
                        if icons::button(ui, "zoom-in", 32.0, false, "Zoom in (⌘+)").clicked() {
                            self.zoom_by(0.1);
                        }
                        ui.label(RichText::new(format!("{:.0}%", self.settings.zoom * 100.0)).color(t.text_muted).font(theme::medium(12.5)));
                        if icons::button(ui, "zoom-out", 32.0, false, "Zoom out (⌘−)").clicked() {
                            self.zoom_by(-0.1);
                        }
                    });
                });
            });
    }

    fn main_menu(&mut self, ui: &mut egui::Ui) {
        let response = widgets::ghost_button(ui, "panel-left", "Menu");
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_width(220.0);
            if ui.button("Open a PDF…").clicked() {
                self.open_dialog();
                ui.close();
            }
            ui.add_enabled_ui(self.doc.is_some(), |ui| {
                if ui.button("Close").clicked() {
                    self.close_document();
                    ui.close();
                }
            });
            ui.separator();
            ui.label(RichText::new("Zoom").small().color(Tokens::get(ui.ctx()).text_faint));
            if ui.button("Zoom in").clicked() {
                self.zoom_by(0.1);
                ui.close();
            }
            if ui.button("Zoom out").clicked() {
                self.zoom_by(-0.1);
                ui.close();
            }
            if ui.button("Actual size (100%)").clicked() {
                self.settings.zoom = 1.0;
                ui.close();
            }
            if ui.button("Fit to width").clicked() {
                self.fit_width = true;
                ui.close();
            }
            ui.separator();
            ui.label(RichText::new("Target language").small().color(Tokens::get(ui.ctx()).text_faint));
            let mut chosen: Option<&str> = None;
            for (code, name) in LANGUAGES {
                if ui.radio(self.settings.target == *code, *name).clicked() {
                    chosen = Some(code);
                }
            }
            if let Some(code) = chosen {
                self.settings.target = code.to_string();
                ui.close();
                if let Some(path) = self.doc.as_ref().map(|doc| doc.path.clone()) {
                    let ctx = ui.ctx().clone();
                    self.open(path, &ctx);
                }
            }
            ui.separator();
            ui.label(RichText::new("Model").small().color(Tokens::get(ui.ctx()).text_faint));
            let previous = self.settings.model.clone();
            for model in MODELS {
                ui.selectable_value(&mut self.settings.model, (*model).to_string(), *model);
            }
            if self.settings.model != previous {
                let ctx = ui.ctx().clone();
                self.respawn_worker(&ctx);
            }
            ui.separator();
            if ui.button("Keyboard shortcuts").clicked() {
                self.shortcuts = true;
                ui.close();
            }
        });
    }

    fn language_button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut chosen: Option<&str> = None;
        ui.menu_button(format!("→ {}", self.target_name()), |ui| {
            for (code, name) in LANGUAGES {
                if ui.radio(self.settings.target == *code, *name).clicked() {
                    chosen = Some(code);
                }
            }
        })
        .response
        .on_hover_text("Target language");
        if let Some(code) = chosen {
            self.settings.target = code.to_string();
            if let Some(path) = self.doc.as_ref().map(|doc| doc.path.clone()) {
                self.open(path, ctx);
            }
        }
    }

    fn model_button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let previous = self.settings.model.clone();
        ui.menu_button(self.settings.model.clone(), |ui| {
            for model in MODELS {
                ui.selectable_value(&mut self.settings.model, (*model).to_string(), *model);
            }
        })
        .response
        .on_hover_text("Translation model");
        if self.settings.model != previous {
            self.respawn_worker(ctx);
        }
    }

    fn left_rail(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let t = Tokens::get(ctx);
        egui::Panel::left("left_rail")
            .exact_size(48.0)
            .frame(Frame::NONE.fill(t.chrome).inner_margin(Margin::symmetric(6, 8)).stroke(Stroke::new(1.0, t.divider)))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                if icons::button(ui, "hand", 36.0, self.pan, "Pan (drag the page)").clicked() {
                    self.pan = !self.pan;
                }
                if icons::button(ui, "zoom-out", 36.0, false, "Zoom out").clicked() {
                    self.zoom_by(-0.1);
                }
                if icons::button(ui, "zoom-in", 36.0, false, "Zoom in").clicked() {
                    self.zoom_by(0.1);
                }
                if icons::button(ui, "maximize", 36.0, false, "Fit to width").clicked() {
                    self.fit_width = true;
                }
                if icons::button(ui, "gauge", 36.0, false, "Actual size (100%)").clicked() {
                    self.settings.zoom = 1.0;
                }
            });
    }

    fn right_rail(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let t = Tokens::get(ctx);
        egui::Panel::right("rail")
            .exact_size(48.0)
            .frame(Frame::NONE.fill(t.chrome).inner_margin(Margin::symmetric(6, 8)).stroke(Stroke::new(1.0, t.divider)))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for (panel, icon, tip) in [
                    (RightPanel::Outline, "list", "Outline"),
                    (RightPanel::Translations, "languages", "Translations"),
                    (RightPanel::Log, "square-terminal", "Log"),
                ] {
                    if icons::button(ui, icon, 36.0, self.right == Some(panel), tip).clicked() {
                        self.right = if self.right == Some(panel) { None } else { Some(panel) };
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                    if icons::button(ui, "circle-help", 36.0, self.shortcuts, "Keyboard shortcuts").clicked() {
                        self.shortcuts = true;
                    }
                    if icons::button(ui, "info", 36.0, false, "About").clicked() {
                        self.toast("PDF Translate — local-model paragraph translations");
                    }
                });
            });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, doc: &Doc, panel: RightPanel, ctx: &egui::Context) {
        let t = Tokens::get(ctx);
        egui::Panel::right("side_panel")
            .exact_size(300.0)
            .resizable(false)
            .frame(Frame::NONE.fill(t.panel).inner_margin(Margin::symmetric(12, 10)).stroke(Stroke::new(1.0, t.divider)))
            .show(ui, |ui| match panel {
                RightPanel::Outline => outline_panel(ui, doc, &mut self.goto),
                RightPanel::Translations => translations_panel(ui, doc, &mut self.filter, &mut self.goto),
                RightPanel::Log => log_panel(ui, self.worker.as_ref()),
            });
    }

    fn palette(&mut self, ctx: &egui::Context) {
        if !self.palette {
            return;
        }
        let t = Tokens::get(ctx);
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.palette = false;
            self.palette_query.clear();
            return;
        }
        let query = self.palette_query.trim().to_lowercase();
        let mut hits: Vec<(usize, Command)> = self
            .commands()
            .into_iter()
            .filter_map(|command| score(command.label, &query).map(|rank| (rank, command)))
            .collect();
        hits.sort_by_key(|(rank, _)| *rank);
        hits.truncate(10);

        let screen = ctx.content_rect();
        let mut chosen: Option<Cmd> = None;
        egui::Area::new(Id::new("palette"))
            .order(egui::Order::Foreground)
            .pivot(Align2::CENTER_TOP)
            .fixed_pos(Pos2::new(screen.center().x, screen.top() + 96.0))
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(520.0);
                        ui.horizontal(|ui| {
                            ui.add(icons::image("search", 18.0, t.text_muted));
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut self.palette_query)
                                    .hint_text("Search tools and commands…")
                                    .frame(Frame::NONE)
                                    .font(theme::regular(15.0))
                                    .desired_width(f32::INFINITY),
                            );
                            if self.palette_focus {
                                edit.request_focus();
                                self.palette_focus = false;
                            }
                            // Enter runs the top hit; the field keeps focus.
                            if (edit.has_focus() || edit.lost_focus()) && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                                if let Some((_, command)) = hits.first() {
                                    chosen = Some(command.cmd);
                                }
                            }
                            edit.request_focus();
                        });
                        ui.separator();
                        for (_, command) in &hits {
                            let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
                            if response.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
                            }
                            icons::paint(ui, Rect::from_min_size(rect.min + vec2(8.0, 9.0), vec2(18.0, 18.0)), command.icon, 17.0, t.icon);
                            ui.painter().text(rect.left_center() + vec2(36.0, 0.0), Align2::LEFT_CENTER, command.label, theme::regular(13.5), t.text);
                            ui.painter().text(rect.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, command.detail, theme::regular(12.0), t.text_faint);
                            if response.clicked() {
                                chosen = Some(command.cmd);
                            }
                        }
                        if hits.is_empty() {
                            ui.label(RichText::new("No matching commands").color(t.text_muted));
                        }
                        ui.add_space(2.0);
                    });
            });
        if let Some(command) = chosen {
            self.palette = false;
            self.palette_query.clear();
            self.run_command(command);
        }
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        if !self.shortcuts {
            return;
        }
        let mut open = true;
        egui::Window::new("Keyboard shortcuts").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            for (keys, what) in [
                ("⌘O", "Open a PDF"),
                ("⌘K", "Command palette"),
                ("⌘+ / ⌘−", "Zoom in / out"),
                ("⌘0", "Actual size (100%)"),
                ("⌘2", "Fit to width"),
                ("Esc", "Close the palette"),
            ] {
                ui.horizontal(|ui| {
                    ui.add_sized([90.0, 18.0], egui::Label::new(RichText::new(keys).monospace().strong()));
                    ui.label(what);
                });
            }
        });
        self.shortcuts = open;
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some((message, start)) = self.toast.clone() else { return };
        let now = ctx.input(|input| input.time);
        let start = if start == 0.0 { now } else { start };
        self.toast = Some((message.clone(), start));
        if now - start > 3.5 {
            self.toast = None;
            return;
        }
        let t = Tokens::get(ctx);
        egui::Area::new(Id::new("toast"))
            .order(egui::Order::Tooltip)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -28.0))
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(if t.dark() { Color32::from_rgb(0xEC, 0xEC, 0xEF) } else { Color32::from_rgb(0x2A, 0x2A, 0x2F) })
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        let color = if t.dark() { Color32::from_rgb(0x22, 0x22, 0x26) } else { Color32::WHITE };
                        ui.label(RichText::new(message).color(color));
                    });
            });
    }

    // ---------------------------------------------------------------- pages

    fn viewer(&mut self, ui: &mut egui::Ui, doc: &mut Doc) {
        let t = Tokens::get(ui.ctx());
        let page_count = doc.info.pages.len();
        // Fit to width before working out where to scroll, so a jump lands on the page.
        if self.fit_width {
            let band = ui.available_width().max(64.0);
            let widest = doc.info.pages.iter().map(|page| page.width).fold(1.0_f32, f32::max);
            self.settings.zoom = ((band - 32.0) / widest).clamp(0.25, 6.0);
            self.fit_width = false;
        }
        let goto_page = self.goto.take();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let band = ui.available_width().max(64.0);
            if self.pan && ui.input(|input| input.pointer.primary_down()) {
                let delta = ui.input(|input| input.pointer.delta());
                ui.scroll_with_delta(-delta);
            }
            let zoom = self.settings.zoom;
            let scale = zoom * ui.ctx().pixels_per_point();
            let clip = ui.clip_rect();
            let pointer = ui.ctx().pointer_hover_pos();
            let mut geoms: Vec<PageGeom> = Vec::with_capacity(page_count);
            let mut hovered: Option<(usize, usize)> = None;

            for page in 0..page_count {
                let info = &doc.info.pages[page];
                let display = Vec2::new((info.width * zoom).max(1.0), (info.height * zoom).max(1.0));
                let (band_rect, _) = ui.allocate_exact_size(Vec2::new(band, display.y + PAGE_GAP), Sense::hover());
                if goto_page == Some(page) {
                    ui.scroll_to_rect(band_rect, Some(Align::TOP));
                }
                let origin = band_rect.min + Vec2::new(((band - display.x) / 2.0).max(0.0), 0.0);
                let page_rect = Rect::from_min_size(origin, display);
                let visible = clip.expand(clip.height()).intersects(band_rect);
                if visible {
                    doc.client.ensure_text(page);
                    doc.client.ensure_pixels(page, scale);
                } else if !clip.expand(clip.height() * 3.0).intersects(band_rect) {
                    doc.client.free_texture(page);
                }

                let painter = ui.painter_at(band_rect);
                painter.add(egui::epaint::Shadow { offset: [0, 3], blur: 14, spread: 0, color: t.page_shadow }.as_shape(page_rect, CornerRadius::ZERO));
                painter.rect_filled(page_rect, CornerRadius::ZERO, Color32::WHITE);
                match (&doc.client.slots[page].texture, doc.client.slots[page].error.as_deref()) {
                    (Some(texture), _) => {
                        painter.image(texture.id(), page_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    }
                    (None, Some(error)) => {
                        painter.text(page_rect.center(), Align2::CENTER_CENTER, error, theme::regular(13.0), Color32::from_rgb(0xBE, 0x46, 0x3C));
                    }
                    (None, None) => {
                        painter.text(page_rect.center(), Align2::CENTER_CENTER, format!("page {} …", page + 1), theme::regular(13.0), t.text_faint);
                    }
                }
                painter.rect_stroke(page_rect, CornerRadius::ZERO, Stroke::new(0.5, t.border.gamma_multiply(0.8)), StrokeKind::Outside);

                let mut paragraph_rects = Vec::new();
                if let Some(paragraphs) = &doc.client.slots[page].paragraphs {
                    for (index, paragraph) in paragraphs.iter().enumerate() {
                        let rect = to_screen(page_rect.min, zoom, paragraph.rect);
                        paragraph_rects.push(rect);
                        let translated = doc.store.lock().map(|store| store.get(&paragraph.key()).is_some()).unwrap_or(false);
                        if translated {
                            // A translated paragraph is marked by the accent bar only, so the mark
                            // can never be mistaken for the hover selection.
                            painter.rect_filled(Rect::from_min_size(Pos2::new(rect.left() - 5.0, rect.top()), Vec2::new(3.0, rect.height())), 1.5, t.accent);
                        }
                        if visible && pointer.is_some_and(|pointer| rect.expand(1.0).contains(pointer)) {
                            hovered = Some((page, index));
                        }
                    }
                }
                geoms.push(PageGeom { page, band: band_rect, page_rect, paragraphs: paragraph_rects });
            }

            if let Some((page, index)) = hovered {
                if let Some(geom) = geoms.iter().find(|geom| geom.page == page) {
                    if let Some(rect) = geom.paragraphs.get(index) {
                        ui.painter_at(geom.band).rect_filled(*rect, 3.0, hover_fill());
                    }
                }
            }

            // The card and the click-to-translate chip stay open while the pointer is on them;
            // the chip keeps its paragraph even where it overlaps another one.
            let over = |rect: Option<Rect>| rect.is_some_and(|rect| pointer.is_some_and(|pointer| rect.contains(pointer)));
            let over_chip = over(self.chip_rect);
            let over_card = over(self.card_rect);
            if !over_chip {
                if hovered.is_some() {
                    self.active = hovered;
                } else if !over_card {
                    self.active = None;
                }
            }

            // Requests in flight: a small chip beside every paragraph being translated.
            for geom in &geoms {
                let Some(paragraphs) = &doc.client.slots[geom.page].paragraphs else { continue };
                for (index, paragraph) in paragraphs.iter().enumerate() {
                    let Some(stage) = self.stages.get(&paragraph.key()) else { continue };
                    if matches!(stage, Stage::Failed(_)) {
                        continue;
                    }
                    if let Some(rect) = geom.paragraphs.get(index) {
                        draw_chip(ui, geom.band, *rect, stage.label(), true, &t);
                    }
                }
            }

            self.card_rects.clear();
            self.card_rect = None;
            self.chip_rect = None;
            if let Some((page, index)) = self.active {
                let paragraph = doc.client.slots.get(page).and_then(|slot| slot.paragraphs.as_ref()).and_then(|paragraphs| paragraphs.get(index)).cloned();
                if let (Some(paragraph), Some(geom)) = (paragraph, geoms.iter().find(|geom| geom.page == page)) {
                    if let Some(rect) = geom.paragraphs.get(index).copied() {
                        let key = paragraph.key();
                        let translation = doc.store.lock().ok().and_then(|store| store.get(&key).map(str::to_string));
                        let running = self.stages.get(&key).is_some_and(|stage| !matches!(stage, Stage::Failed(_)));
                        if let Some(translation) = translation {
                            // Reading a translation: the card, in the paragraph's own format.
                            if !self.dismissed.contains(&(page, index)) {
                                let (card_rect, action) =
                                    card_ui(ui, geom.band, geom.page_rect, rect, doc.face, &paragraph, &translation, &self.settings.target, zoom, page, index);
                                self.card_rect = Some(card_rect);
                                self.card_rects.push(card_rect);
                                if matches!(action, CardAction::Dismiss) {
                                    self.dismissed.insert((page, index));
                                    self.active = None;
                                }
                            }
                        } else if !running {
                            // Nothing to read yet: only a chip that asks for a translation.
                            let label = match self.stages.get(&key) {
                                Some(Stage::Failed(_)) => "Translation failed — click to retry",
                                _ => "Click to translate",
                            };
                            self.chip_rect = Some(draw_chip(ui, geom.band, rect, label, false, &t));
                        }
                    }
                }
            }

            // Clicks: a paragraph (or its chip) asks for a translation; a translated
            // paragraph opens its card again.
            let clicked = ui.input(|input| input.pointer.primary_clicked());
            let click_pos = ui.input(|input| input.pointer.interact_pos());
            if clicked && !self.pan {
                if let Some(position) = click_pos {
                    let on_chip = self.chip_rect.is_some_and(|rect| rect.contains(position));
                    let over_card = self.card_rects.iter().any(|rect| rect.contains(position));
                    let hit = if on_chip {
                        self.active
                    } else if over_card {
                        None
                    } else {
                        hit_test(&geoms, &doc.client.slots, position)
                    };
                    if let Some(hit) = hit {
                        self.dismissed.remove(&hit);
                        self.active = Some(hit);
                        let paragraph = doc.client.slots[hit.0].paragraphs.as_ref().and_then(|paragraphs| paragraphs.get(hit.1)).cloned();
                        if let Some(paragraph) = paragraph {
                            let key = paragraph.key();
                            let store = doc.store.clone();
                            self.request(&store, key, paragraph.text.clone(), hit.0);
                        }
                    }
                }
            }
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.28);
            ui.label(RichText::new("PDF Translate").font(theme::semibold(30.0)));
            ui.add_space(8.0);
            ui.label(RichText::new("Open a PDF, hover a paragraph, read its translation beside it.").color(t.text_muted));
            ui.add_space(18.0);
            if ui.add(egui::Button::new(RichText::new("Open a PDF…").color(Color32::WHITE).font(theme::medium(13.5))).fill(t.accent).corner_radius(CornerRadius::same(14)).min_size(vec2(150.0, 32.0))).clicked() {
                self.open_dialog();
            }
            if let Some(error) = &self.error {
                ui.add_space(12.0);
                ui.colored_label(Color32::from_rgb(0xBE, 0x46, 0x3C), error);
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // A file chosen in a dialog is opened here, before any panel takes the document out.
        if let Some(path) = self.pending_open.take() {
            self.open(path, &ctx);
        }
        let kind = self.settings.theme.resolve(ctx.system_theme(), ThemeKind::Light);
        if kind != self.kind {
            theme::apply(&ctx, kind);
            self.kind = kind;
        }
        self.poll_updates();
        let command = ctx.input(|input| input.modifiers.command);
        if command && ctx.input(|input| input.key_pressed(egui::Key::O)) {
            self.open_dialog();
        }
        if command && ctx.input(|input| input.key_pressed(egui::Key::K)) {
            self.palette = true;
            self.palette_focus = true;
        }
        if command && ctx.input(|input| input.key_pressed(egui::Key::Plus)) {
            self.zoom_by(0.1);
        }
        if command && ctx.input(|input| input.key_pressed(egui::Key::Minus)) {
            self.zoom_by(-0.1);
        }
        if command && ctx.input(|input| input.key_pressed(egui::Key::Num0)) {
            self.settings.zoom = 1.0;
        }
        if command && ctx.input(|input| input.key_pressed(egui::Key::Num2)) {
            self.fit_width = true;
        }

        self.tab_strip(ui, &ctx);
        self.mode_bar(ui, &ctx);
        let mut doc = self.doc.take();
        if let Some(doc) = doc.as_mut() {
            doc.client.poll(&ctx);
            self.left_rail(ui, &ctx);
            self.right_rail(ui, &ctx);
            if let Some(panel) = self.right {
                self.side_panel(ui, doc, panel, &ctx);
            }
        }
        let t = Tokens::get(&ctx);
        let fill = t.pasteboard;
        match doc.as_mut() {
            Some(doc) => {
                egui::CentralPanel::default().frame(Frame::NONE.fill(fill)).show(ui, |ui| self.viewer(ui, doc));
            }
            None => {
                egui::CentralPanel::default().frame(Frame::NONE.fill(fill)).show(ui, |ui| self.welcome(ui));
            }
        }
        // If a panel opened a document while the old one was taken out, keep the new one.
        if self.doc.is_none() {
            self.doc = doc;
        }
        self.palette(&ctx);
        self.shortcuts_window(&ctx);
        self.toast_ui(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }
}

/// The single open document as a tab in the title strip.
fn document_tab(ui: &mut egui::Ui, t: &Tokens, name: &str) -> bool {
    let font = theme::regular(13.0);
    let label: String = if name.chars().count() > 28 { format!("{}…", name.chars().take(27).collect::<String>()) } else { name.to_string() };
    let text_w = ui.fonts_mut(|f| f.layout_no_wrap(label.clone(), font.clone(), t.text).size().x);
    let (rect, _) = ui.allocate_exact_size(vec2(text_w + 64.0, 30.0), Sense::click());
    ui.painter().rect_filled(rect, CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }, t.chrome);
    icons::paint(ui, Rect::from_min_size(rect.min + vec2(6.0, 7.0), vec2(16.0, 16.0)), "file-text", 15.0, t.accent);
    ui.painter().text(rect.min + vec2(28.0, rect.height() / 2.0), Align2::LEFT_CENTER, label, font, t.text);
    let x_rect = Rect::from_center_size(rect.right_center() - vec2(16.0, 0.0), vec2(20.0, 20.0));
    let x = ui.interact(x_rect, ui.id().with("tab_close"), Sense::click());
    if x.hovered() {
        ui.painter().rect_filled(x_rect, CornerRadius::same(4), t.pressed);
    }
    icons::paint(ui, x_rect, "x", 13.0, t.text_muted);
    x.clicked()
}

fn outline_panel(ui: &mut egui::Ui, doc: &Doc, goto: &mut Option<usize>) {
    widgets::section_title(ui, "Outline");
    if doc.info.outline.is_empty() {
        ui.label(RichText::new("This document has no bookmarks.").weak());
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| outline_items(ui, &doc.info.outline, goto, 0));
}

fn outline_items(ui: &mut egui::Ui, items: &[OutlineItem], goto: &mut Option<usize>, depth: usize) {
    let t = Tokens::get(ui.ctx());
    for item in items {
        let response = ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 12.0);
            ui.selectable_label(false, RichText::new(&item.title).color(if item.page.is_some() { t.text } else { t.text_faint }))
        });
        if response.inner.clicked() {
            if let Some(page) = item.page {
                *goto = Some(page);
            }
        }
        if !item.children.is_empty() {
            outline_items(ui, &item.children, goto, depth + 1);
        }
    }
}

fn translations_panel(ui: &mut egui::Ui, doc: &Doc, filter: &mut String, goto: &mut Option<usize>) {
    widgets::section_title(ui, "Translations");
    ui.add(egui::TextEdit::singleline(filter).hint_text("Filter…").desired_width(f32::INFINITY));
    ui.add_space(6.0);
    let entries = doc.store.lock().map(|store| store.entries()).unwrap_or_default();
    ui.label(RichText::new(format!("{} translated paragraph(s)", entries.len())).weak().small());
    ui.add_space(4.0);
    let query = filter.to_lowercase();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut shown = 0;
        for (key, source, translation, page) in &entries {
            if !query.is_empty() && !source.to_lowercase().contains(&query) && !translation.to_lowercase().contains(&query) {
                continue;
            }
            shown += 1;
            let row = ui.vertical(|ui| {
                ui.add_space(6.0);
                ui.label(RichText::new(translation).font(theme::regular(13.0)));
                ui.label(RichText::new(shorten(source, 90)).weak().small());
            });
            if ui.interact(row.response.rect, ui.id().with(("translation", key)), Sense::click()).on_hover_text("Show this page").clicked() {
                if let Some(page) = page.or_else(|| page_of_key(doc, key)) {
                    *goto = Some(page);
                }
            }
            ui.separator();
        }
        if shown == 0 {
            ui.label(RichText::new("Nothing translated yet. Hover a paragraph and click to translate it.").weak());
        }
    });
}

fn log_panel(ui: &mut egui::Ui, worker: Option<&Worker>) {
    widgets::section_title(ui, "Log");
    let lines = worker.map(|worker| worker.log.lock().map(|log| log.clone()).unwrap_or_default()).unwrap_or_default();
    if lines.is_empty() {
        ui.label(RichText::new("Nothing logged yet. The model's messages appear here.").weak());
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for line in lines.iter().rev() {
            ui.label(RichText::new(line).monospace().small());
        }
    });
}

/// The page a translated paragraph sits on, among the pages whose text is loaded.
fn page_of_key(doc: &Doc, key: &str) -> Option<usize> {
    doc.client.slots.iter().position(|slot| slot.paragraphs.as_ref().is_some_and(|paragraphs| paragraphs.iter().any(|paragraph| paragraph.key() == key)))
}

fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(limit).collect::<String>())
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn to_screen(origin: Pos2, zoom: f32, rect: [f32; 4]) -> Rect {
    Rect::from_min_max(origin + Vec2::new(rect[0] * zoom, rect[1] * zoom), origin + Vec2::new(rect[2] * zoom, rect[3] * zoom))
}

fn hit_test(geoms: &[PageGeom], slots: &[PageSlot], position: Pos2) -> Option<(usize, usize)> {
    for geom in geoms {
        if !geom.page_rect.expand(1.0).contains(position) {
            continue;
        }
        let paragraphs = slots.get(geom.page).and_then(|slot| slot.paragraphs.as_ref())?;
        for (index, rect) in geom.paragraphs.iter().enumerate() {
            if index < paragraphs.len() && rect.expand(1.0).contains(position) {
                return Some((geom.page, index));
            }
        }
    }
    None
}

/// The small chip beside a paragraph: what a running request is doing, or the prompt to ask
/// for one. The spinner turns only while a request is running. Returns the chip's rectangle,
/// so a click on it is not taken for a click on the page.
fn draw_chip(ui: &egui::Ui, band: Rect, paragraph: Rect, label: &str, spinning: bool, t: &Tokens) -> Rect {
    let painter = ui.painter_at(band);
    let font = theme::regular(12.0);
    let galley = painter.layout_no_wrap(label.to_string(), font, t.text_muted);
    // The spinner's slot (only while translating), then padding around the label.
    let slot = if spinning { CHIP_SPINNER } else { 0.0 };
    let size = galley.size() + Vec2::new(CHIP_PADDING * 2.0 + slot, CHIP_PADDING * 2.0);
    let mut x = paragraph.right() + 6.0;
    if x + size.x > band.right() - 4.0 {
        x = paragraph.left() - size.x - 6.0;
    }
    let y = paragraph.top();
    let chip = Rect::from_min_size(Pos2::new(x.max(band.left() + 4.0), y), size);
    painter.rect_filled(chip, 10.0, t.card);
    painter.rect_stroke(chip, 10.0, Stroke::new(1.0, t.border), StrokeKind::Outside);
    if spinning {
        // A little arc spinner, since this is painted, not a widget.
        let time = ui.input(|input| input.time) as f32;
        let center = Pos2::new(chip.left() + CHIP_PADDING + 6.0, chip.center().y);
        let start = time * 5.0;
        let points: Vec<Pos2> = (0..=18)
            .map(|step| {
                let angle = start + (step as f32 / 18.0) * std::f32::consts::TAU * 0.75;
                center + Vec2::new(angle.cos(), angle.sin()) * 6.0
            })
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(2.0, t.accent)));
        ui.ctx().request_repaint();
    }
    painter.galley(Pos2::new(chip.left() + CHIP_PADDING + slot, chip.center().y - galley.size().y / 2.0), galley, t.text_muted);
    chip
}

/// The card beside a paragraph: shown only when there is a translation to read.
#[allow(clippy::too_many_arguments)]
fn card_ui(
    ui: &mut egui::Ui,
    band: Rect,
    page_rect: Rect,
    paragraph_rect: Rect,
    face: Typeface,
    paragraph: &Paragraph,
    translation: &str,
    target: &str,
    zoom: f32,
    page: usize,
    index: usize,
) -> (Rect, CardAction) {
    let t = Tokens::get(ui.ctx());
    // Put the card on the side of the paragraph that has the most room on the page, so it
    // never covers the text it translates when there is a margin to use.
    let space_left = paragraph_rect.left() - page_rect.left();
    let space_right = page_rect.right() - paragraph_rect.right();
    let to_the_right = space_right >= space_left;
    // As wide as the paragraph when that side has room, so the translation keeps its measure.
    let room = if to_the_right { band.right() - paragraph_rect.right() } else { paragraph_rect.left() - band.left() };
    let card_width = paragraph_rect.width().clamp(200.0, 640.0).min((room - CARD_GAP - 6.0).max(200.0)).min((band.width() - 16.0).max(200.0));
    let mut x = if to_the_right { paragraph_rect.right() + CARD_GAP } else { paragraph_rect.left() - CARD_GAP - card_width };
    x = x.clamp(band.left() + 6.0, (band.right() - card_width - 6.0).max(band.left() + 6.0));
    let y = (paragraph_rect.top() - 6.0).max(band.top() + 4.0);
    let outer = Rect::from_min_size(Pos2::new(x, y), Vec2::new(card_width, (band.bottom() - y).max(120.0)));

    let mut action = CardAction::None;
    let style = ui.style().clone();
    let mut child = ui.new_child(UiBuilder::new().id_salt(Id::new(("translation-card", page, index))).max_rect(outer).layout(Layout::top_down(Align::Min)));
    let frame = Frame::popup(&style).inner_margin(Margin::same(CARD_PADDING as i8)).show(&mut child, |ui| {
        ui.set_max_width(card_width - CARD_PADDING * 2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("→ {target}")).small().color(t.accent)).on_hover_text(format!("{} line(s), {:.0} pt", paragraph.lines.len(), paragraph.size));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icons::button(ui, "x", 22.0, false, "Hide this translation").clicked() {
                    action = CardAction::Dismiss;
                }
            });
        });
        ui.add_space(2.0);
        // Same type as the paragraph: its size, leading and typeface, its run-in bold.
        let size = (paragraph.size * zoom).clamp(11.0, 26.0);
        let bold = run_in(&paragraph.text).and_then(|_| run_in(translation));
        let mut job = document_job(translation, size, size * paragraph.line_spacing, face, bold, t.text);
        job.wrap.max_width = card_width - CARD_PADDING * 2.0 - 4.0;
        ui.add(egui::Label::new(job));
    });
    (frame.response.rect, action)
}

/// A layout job for text quoted or translated from the document: the document's typeface,
/// the paragraph's size and leading, its run-in heading in bold.
fn document_job(text: &str, size: f32, line_height: f32, face: Typeface, bold_prefix: Option<usize>, color: Color32) -> LayoutJob {
    let (regular, bold) = document_fonts(face, size);
    let format = |font_id| TextFormat { font_id, color, line_height: Some(line_height), ..Default::default() };
    let mut job = LayoutJob::default();
    match bold_prefix.filter(|end| *end <= text.len() && text.is_char_boundary(*end)) {
        Some(end) => {
            job.append(&text[..end], 0.0, format(bold));
            job.append(&text[end..], 0.0, format(regular));
        }
        None => job.append(text, 0.0, format(regular)),
    }
    job
}

/// The regular and bold faces of a document's family.
fn document_fonts(face: Typeface, size: f32) -> (egui::FontId, egui::FontId) {
    match face {
        Typeface::Serif => (theme::serif(size), theme::serif_bold(size)),
        Typeface::Sans => (theme::regular(size), theme::semibold(size)),
        Typeface::Mono => (egui::FontId::monospace(size), egui::FontId::monospace(size)),
    }
}

/// The byte length of a paragraph's run-in heading, colon included ("Müller:", "SPIEGEL:
/// Mediatorin:"), or `None` when the text does not start with one. A run-in is set bold in
/// the source, so the matching label is set bold in the translation too.
fn run_in(text: &str) -> Option<usize> {
    let limit = text.char_indices().nth(40).map_or(text.len(), |(index, _)| index);
    let colon = text[..limit].find(':')?;
    let label = &text[..colon];
    if label.is_empty() || label.chars().count() > 32 || label.split_whitespace().count() > 4 {
        return None;
    }
    if label.chars().any(|c| matches!(c, '.' | '!' | '?' | ';' | ',' | '»' | '«' | '–' | '—')) {
        return None;
    }
    label.chars().next().is_some_and(char::is_uppercase).then_some(colon + 1)
}
