//! Primary and secondary text buttons (adapted from the visual reference;
//! MIT OR Apache-2.0 — see ATTRIBUTION.md).

use egui::{Color32, CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::ui::theme;

/// The three button fills: resting, hovered, pressed.
struct Fills {
    rest: Color32,
    hover: Color32,
    pressed: Color32,
}

fn paint_button(
    ui: &Ui,
    response: &Response,
    rect: egui::Rect,
    text: &str,
    fills: &Fills,
    fg: Color32,
    border: Stroke,
) {
    let t = theme::tokens(ui.ctx());
    let fill = if response.is_pointer_button_down_on() {
        fills.pressed
    } else if response.hovered() {
        fills.hover
    } else {
        fills.rest
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

fn button(ui: &mut Ui, text: &str, fills: &Fills, fg: Color32, border: Stroke) -> Response {
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
        paint_button(ui, &response, rect, text, fills, fg, border);
    }
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), text));
    response
}

/// Filled call-to-action button (`primary_bg` / `primary_text`): hover
/// and pressed keep the accent hue — just lighter/darker, not the
/// neutral grey that read as a washed-out button.
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let t = theme::tokens(ui.ctx());
    button(
        ui,
        text,
        &Fills {
            rest: t.primary_bg,
            hover: t.primary_bg.blend(Color32::from_white_alpha(40)),
            pressed: t.primary_bg.blend(Color32::from_black_alpha(45)),
        },
        t.primary_text,
        Stroke::NONE,
    )
}

/// Quiet button on `field` with `field_border` outline.
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    let t = theme::tokens(ui.ctx());
    button(
        ui,
        text,
        &Fills {
            rest: t.field,
            hover: t.hover,
            pressed: t.pressed,
        },
        t.text,
        Stroke::new(1.0, t.field_border),
    )
}
