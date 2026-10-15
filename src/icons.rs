//! Vector icons: the official Lucide SVGs, embedded from the `lucide-static-svg` crate
//! (Lucide is ISC-licensed), recoloured and rasterized by `egui_extras`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use egui::{Color32, Rect, Response, Sense, Vec2};
use lucide_static_svg::Icon;

use crate::theme::Tokens;

/// The icons the chrome uses, under the names the rest of the app asks for them by.
static ICONS: &[(&str, Icon)] = &[
    ("house", Icon::House),
    ("file-text", Icon::FileText),
    ("x", Icon::X),
    ("plus", Icon::Plus),
    ("sun", Icon::Sun),
    ("moon", Icon::Moon),
    ("settings", Icon::Settings),
    // Lucide renamed these two ("help-circle" → "circle-question-mark", "trash-2" → "trash");
    // the UI keeps its shorter names.
    ("circle-help", Icon::CircleQuestionMark),
    ("panel-left", Icon::PanelLeft),
    ("search", Icon::Search),
    ("hand", Icon::Hand),
    ("zoom-in", Icon::ZoomIn),
    ("zoom-out", Icon::ZoomOut),
    ("maximize", Icon::Maximize),
    ("gauge", Icon::Gauge),
    ("square", Icon::Square),
    ("layout", Icon::LayoutTemplate),
    ("list", Icon::List),
    ("languages", Icon::Languages),
    ("square-terminal", Icon::SquareTerminal),
    ("info", Icon::Info),
    ("check", Icon::Check),
    ("book-open", Icon::BookOpen),
    ("chevron-up", Icon::ChevronUp),
    ("chevron-down", Icon::ChevronDown),
    ("message-square", Icon::MessageSquare),
    ("external-link", Icon::ExternalLink),
    ("trash-2", Icon::Trash),
];

fn white_icons() -> &'static HashMap<&'static str, Arc<[u8]>> {
    static MAP: OnceLock<HashMap<&'static str, Arc<[u8]>>> = OnceLock::new();
    MAP.get_or_init(|| {
        ICONS
            .iter()
            .map(|(name, icon)| {
                // egui rasters the SVG at the tint's colour, so the stroke colour is patched
                // here, and the stroke is a touch lighter than Lucide's default.
                let svg = icon.svg_str().replace("currentColor", "#ffffff").replace("stroke-width=\"2\"", "stroke-width=\"1.75\"");
                (*name, Arc::from(svg.into_bytes().into_boxed_slice()))
            })
            .collect()
    })
}


pub fn image(name: &str, size: f32, tint: Color32) -> egui::Image<'static> {
    // An unknown name draws the empty square, so a missing icon is visible but harmless.
    let (key, bytes) = match white_icons().get(name) {
        Some(bytes) => (name, bytes.clone()),
        None => ("square", white_icons().get("square").cloned().unwrap_or_default()),
    };
    egui::Image::from_bytes(format!("bytes://icons/{key}.svg"), egui::load::Bytes::Shared(bytes)).fit_to_exact_size(Vec2::splat(size)).tint(tint)
}

pub fn paint(ui: &egui::Ui, rect: Rect, name: &str, size: f32, tint: Color32) {
    image(name, size, tint).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(size)));
}

/// Square icon button: transparent until hovered; `selected` gets the accent treatment.
pub fn button(ui: &mut egui::Ui, name: &str, box_size: f32, selected: bool, tooltip: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(box_size), Sense::click());
    let label = if tooltip.is_empty() { name } else { tooltip };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, label));
    if selected {
        ui.painter().rect_filled(rect, t.radius, t.accent_soft);
    } else if resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, t.radius, t.pressed);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, t.radius, t.hover);
    }
    let tint = if selected { t.accent_text } else { t.icon };
    paint(ui, rect, name, (box_size * 0.5).round(), tint);
    if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) }
}
