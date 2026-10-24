//! 1 px hairline separators using the `separator` token.

use egui::{Response, Sense, Ui, Vec2};

use crate::ui::theme;

/// Horizontal hairline across the available width.
pub fn hairline(ui: &mut Ui) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().hline(
            rect.x_range(),
            rect.center().y,
            egui::Stroke::new(1.0, theme::tokens(ui.ctx()).separator),
        );
    }
    response
}

/// Vertical hairline `height` px tall (toolbar separators).
pub fn vline(ui: &mut Ui, height: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(1.0, height), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().vline(
            rect.center().x,
            rect.y_range(),
            egui::Stroke::new(1.0, theme::tokens(ui.ctx()).separator),
        );
    }
    response
}
