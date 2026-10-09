//! Vector icons: the Lucide SVGs PdfCraft uses, recoloured and rasterized by `egui_extras`.
//! The files are read from the PdfCraft checkout (`assets/icons`, ISC licence).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use egui::{Color32, Rect, Response, Sense, Vec2};

use crate::theme::Tokens;

static ICONS: &[(&str, &[u8])] = &[
    ("house", include_bytes!("../../pdfcraft/assets/icons/house.svg")),
    ("file-text", include_bytes!("../../pdfcraft/assets/icons/file-text.svg")),
    ("x", include_bytes!("../../pdfcraft/assets/icons/x.svg")),
    ("plus", include_bytes!("../../pdfcraft/assets/icons/plus.svg")),
    ("sun", include_bytes!("../../pdfcraft/assets/icons/sun.svg")),
    ("moon", include_bytes!("../../pdfcraft/assets/icons/moon.svg")),
    ("settings", include_bytes!("../../pdfcraft/assets/icons/settings.svg")),
    ("circle-help", include_bytes!("../../pdfcraft/assets/icons/circle-help.svg")),
    ("panel-left", include_bytes!("../../pdfcraft/assets/icons/panel-left.svg")),
    ("search", include_bytes!("../../pdfcraft/assets/icons/search.svg")),
    ("hand", include_bytes!("../../pdfcraft/assets/icons/hand.svg")),
    ("zoom-in", include_bytes!("../../pdfcraft/assets/icons/zoom-in.svg")),
    ("zoom-out", include_bytes!("../../pdfcraft/assets/icons/zoom-out.svg")),
    ("maximize", include_bytes!("../../pdfcraft/assets/icons/maximize.svg")),
    ("gauge", include_bytes!("../../pdfcraft/assets/icons/gauge.svg")),
    ("list", include_bytes!("../../pdfcraft/assets/icons/list.svg")),
    ("languages", include_bytes!("../../pdfcraft/assets/icons/languages.svg")),
    ("square-terminal", include_bytes!("../../pdfcraft/assets/icons/square-terminal.svg")),
    ("info", include_bytes!("../../pdfcraft/assets/icons/info.svg")),
    ("check", include_bytes!("../../pdfcraft/assets/icons/check.svg")),
    ("book-open", include_bytes!("../../pdfcraft/assets/icons/book-open.svg")),
    ("chevron-up", include_bytes!("../../pdfcraft/assets/icons/chevron-up.svg")),
    ("chevron-down", include_bytes!("../../pdfcraft/assets/icons/chevron-down.svg")),
    ("message-square", include_bytes!("../../pdfcraft/assets/icons/message-square.svg")),
    ("external-link", include_bytes!("../../pdfcraft/assets/icons/external-link.svg")),
    ("trash-2", include_bytes!("../../pdfcraft/assets/icons/trash-2.svg")),
];

fn white_icons() -> &'static HashMap<&'static str, Arc<[u8]>> {
    static MAP: OnceLock<HashMap<&'static str, Arc<[u8]>>> = OnceLock::new();
    MAP.get_or_init(|| {
        ICONS
            .iter()
            .map(|(name, bytes)| {
                let svg = String::from_utf8_lossy(bytes).replace("currentColor", "#ffffff").replace("stroke-width=\"2\"", "stroke-width=\"1.75\"");
                (*name, Arc::from(svg.into_bytes().into_boxed_slice()))
            })
            .collect()
    })
}

pub fn image(name: &str, size: f32, tint: Color32) -> egui::Image<'static> {
    let (key, bytes) = match white_icons().get(name) {
        Some(bytes) => (name, bytes.clone()),
        None => ("square", white_icons().get("check").cloned().unwrap_or_default()),
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
