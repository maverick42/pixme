//! Primary and secondary text buttons (adapted from the visual reference;
//! MIT OR Apache-2.0 — see ATTRIBUTION.md).

use egui::{Color32, CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::ui::theme;

fn paint_button(
    ui: &Ui,
    response: &Response,
    rect: egui::Rect,
    text: &str,
    fill: Color32,
    fg: Color32,
    border: Stroke,
) {
    let t = theme::tokens(ui.ctx());
    let fill = if response.is_pointer_button_down_on() {
        t.pressed
    } else if response.hovered() {
        // One step brighter/darker than the resting fill.
        t.hover
    } else {
        fill
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(t.radius_md),
        fill,
        border,
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::TextStyle::Button.resolve(ui.style()),
        fg,
    );
}

fn button(ui: &mut Ui, text: &str, fill: Color32, fg: Color32, border: Stroke) -> Response {
    let padding = ui.spacing().button_padding;
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::TextStyle::Button.resolve(ui.style()),
        fg,
    );
    let size = galley.size() + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(size.x.max(size.y), size.y.max(24.0)),
        Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        paint_button(ui, &response, rect, text, fill, fg, border);
    }
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), text));
    response
}

/// Filled call-to-action button (`primary_bg` / `primary_text`).
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let t = theme::tokens(ui.ctx());
    button(ui, text, t.primary_bg, t.primary_text, Stroke::NONE)
}

/// Quiet button on `field` with `field_border` outline.
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    let t = theme::tokens(ui.ctx());
    button(ui, text, t.field, t.text, Stroke::new(1.0, t.field_border))
}
