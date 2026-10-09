//! Chrome widgets copied from PdfCraft (`crates/ui-egui/src/widgets.rs`): mode tabs, ghost
//! buttons, the search-field lookalike and section titles.

use egui::{Align2, CornerRadius, Response, Sense, Stroke, StrokeKind, Vec2, vec2};

use crate::theme::{self, Tokens};
use crate::icons;

/// A text tab of the mode bar: the active one is semibold with a short underline.
pub fn mode_tab(ui: &mut egui::Ui, label: &str, active: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = if active { theme::semibold(13.5) } else { theme::medium(13.5) };
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 22.0, 48.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, label));
    if resp.hovered() && !active {
        ui.painter().rect_filled(rect.shrink2(vec2(2.0, 9.0)), CornerRadius::same(6), t.hover);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, if active { t.text } else { t.text_muted });
    if active {
        let r = egui::Rect::from_min_max(rect.left_bottom() + vec2(11.0, -3.0), rect.right_bottom() - vec2(11.0, 0.0));
        ui.painter().rect_filled(r, CornerRadius::same(1), t.text);
    }
    resp
}


/// An icon with a text label: the tab-strip action ("+ Open").
pub fn ghost_button(ui: &mut egui::Ui, icon: &str, label: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(13.0);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 38.0, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
    }
    icons::paint(ui, egui::Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(18.0, 18.0)), icon, 17.0, t.icon);
    ui.painter().text(rect.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, label, font, t.text);
    resp
}

/// A search-field lookalike that opens the command palette.
pub fn search_box(ui: &mut egui::Ui, placeholder: &str, width: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), placeholder));
    let fill = if resp.hovered() { t.hover } else { t.field };
    ui.painter().rect(rect, CornerRadius::same(16), fill, Stroke::new(1.0, t.border), StrokeKind::Inside);
    icons::paint(ui, egui::Rect::from_min_size(rect.min + vec2(10.0, 8.0), vec2(16.0, 16.0)), "search", 15.0, t.text_muted);
    let shortcut = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::K));
    let shortcut = ui.painter().layout_no_wrap(shortcut, theme::regular(11.5), t.text_faint);
    let shortcut_pos = rect.right_center() - vec2(12.0 + shortcut.size().x, shortcut.size().y * 0.5);
    let text_left = rect.left() + 34.0;
    ui.painter().text(egui::pos2(text_left, rect.center().y), Align2::LEFT_CENTER, placeholder, theme::regular(13.0), t.text_faint);
    ui.painter().galley(shortcut_pos, shortcut, t.text_faint);
    resp.on_hover_cursor(egui::CursorIcon::Text)
}

/// An uppercase, letter-spaced section title inside a panel.
pub fn section_title(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(10.0);
    ui.label(egui::RichText::new(text.to_uppercase()).font(theme::semibold(10.5)).color(t.text_faint).extra_letter_spacing(0.6));
    ui.add_space(2.0);
}

/// A thin vertical divider for a horizontal bar.
pub fn vline(ui: &mut egui::Ui, height: f32, gap: f32) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(gap);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, height), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, t.divider));
    ui.add_space(gap);
}
