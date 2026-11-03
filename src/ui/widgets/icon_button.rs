//! Square icon button in the visual reference's style (adapted; MIT OR
//! Apache-2.0 — see ATTRIBUTION.md): transparent at rest, `hover` fill on
//! hover, `pressed` while down, `accent_soft` + `accent_border` when
//! selected.

use egui::{Color32, CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::ui::icons::{self, Icon};
use crate::ui::theme;

/// 26 px square holding a 16 px icon (toolbar metrics of the reference).
pub const BUTTON_SIZE: f32 = 26.0;
/// Icon side inside the button.
pub const ICON_SIZE: f32 = 16.0;

/// A clickable icon button. `selected` draws the accent-soft fill and border
/// used for active tools.
pub fn icon_button(ui: &mut Ui, icon: Icon, selected: bool) -> Response {
    icon_button_styled(ui, icon, selected, None)
}

/// Same as [`icon_button`] but the icon is painted in `tint` while enabled
/// (e.g. the danger marker on an armed capture delay).
pub fn icon_button_tinted(ui: &mut Ui, icon: Icon, selected: bool, tint: Color32) -> Response {
    icon_button_styled(ui, icon, selected, Some(tint))
}

fn icon_button_styled(ui: &mut Ui, icon: Icon, selected: bool, tint: Option<Color32>) -> Response {
    let t = theme::tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON_SIZE), Sense::click());

    let icon_tint = if !ui.is_enabled() {
        t.text_faint
    } else if let Some(tint) = tint {
        tint
    } else if response.hovered() {
        // Hover pops like `tool_button`: accent icon.
        t.accent
    } else {
        t.icon
    };
    if ui.is_rect_visible(rect) {
        let (fill, stroke) = if response.is_pointer_button_down_on() {
            (t.pressed, Stroke::NONE)
        } else if selected {
            (t.accent_soft, Stroke::new(1.0, t.accent_border))
        } else if response.hovered() {
            (t.hover, Stroke::new(1.0, t.accent_soft))
        } else {
            (Color32::TRANSPARENT, Stroke::NONE)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(t.radius_sm),
            fill,
            stroke,
            StrokeKind::Inside,
        );
        icons::paint(ui, rect, icon, ICON_SIZE, icon_tint);
    }

    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), ""));
    response
}
