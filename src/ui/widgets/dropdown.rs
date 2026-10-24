//! Field-styled dropdown button showing the current value and a chevron.
//! The popup wiring (menu of choices) is added where it is used — this widget
//! is only the painted, clickable field (adapted from the visual reference;
//! MIT OR Apache-2.0 — see ATTRIBUTION.md).

use egui::{Align2, CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::ui::icons::{self, Icon};
use crate::ui::theme;

/// A clickable field showing `text` + chevron; the caller opens a
/// `egui::popup` menu `on` the returned response.
pub fn dropdown_button(ui: &mut Ui, text: &str) -> Response {
    let t = theme::tokens(ui.ctx());
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), t.text);
    let size = Vec2::new(galley.size().x + 36.0, (galley.size().y + 10.0).max(24.0));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    if ui.is_rect_visible(rect) {
        let fill = if response.is_pointer_button_down_on() {
            t.pressed
        } else if response.hovered() {
            t.hover
        } else {
            t.field
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(t.radius_md),
            fill,
            Stroke::new(1.0, t.field_border),
            StrokeKind::Inside,
        );
        ui.painter().text(
            rect.left_center() + egui::vec2(10.0, 0.0),
            Align2::LEFT_CENTER,
            text,
            font,
            t.text,
        );
        icons::paint(
            ui,
            egui::Rect::from_center_size(
                rect.right_center() - egui::vec2(14.0, 0.0),
                Vec2::splat(12.0),
            ),
            Icon::ChevronDown,
            12.0,
            t.icon,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, ui.is_enabled(), text)
    });
    response
}
