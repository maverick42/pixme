//! Square icon button in the visual reference's style (adapted; MIT OR
//! Apache-2.0 — see ATTRIBUTION.md): transparent at rest, `hover` fill on
//! hover, `pressed` while down, `accent_soft` + `accent_border` when
//! selected.

use egui::{Color32, CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::config::IconStyle;
use crate::ui::icons::{self, Icon};
use crate::ui::theme;

/// 26 px square holding a 16 px icon (toolbar metrics of the reference).
pub const BUTTON_SIZE: f32 = 26.0;
/// Icon side inside the button.
pub const ICON_SIZE: f32 = 16.0;

/// A clickable icon button. `selected` draws the accent-soft fill and border
/// used for active tools.
pub fn icon_button(ui: &mut Ui, icon: Icon, selected: bool) -> Response {
    icon_button_impl(ui, icon, selected, None, IconStyle::Standard)
}

/// Same button honouring `style`: `Color` paints the glyph in its own
/// hue, `Duotone` swaps in the Phosphor two-tone glyph. `tint` (the
/// armed-delay marker) and the disabled state still override the style.
pub fn icon_button_styled(
    ui: &mut Ui,
    icon: Icon,
    selected: bool,
    style: IconStyle,
    tint: Option<Color32>,
) -> Response {
    icon_button_impl(ui, icon, selected, tint, style)
}

fn icon_button_impl(
    ui: &mut Ui,
    icon: Icon,
    selected: bool,
    tint: Option<Color32>,
    style: IconStyle,
) -> Response {
    let t = theme::tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON_SIZE), Sense::click());

    // The widget-state tint: disabled and explicit overrides beat the
    // hover accent, which beats the resting icon colour.
    let state_tint = if !ui.is_enabled() {
        t.text_faint
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
        // Disabled and explicit overrides beat the style's own hue,
        // which beats the widget-state tint.
        let icon_tint = if !ui.is_enabled() {
            t.text_faint
        } else {
            tint.or_else(|| icons::hue(icon, style))
                .unwrap_or(state_tint)
        };
        icons::styled_paint(ui, rect, icon, ICON_SIZE, style, icon_tint);
    }

    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), ""));
    response
}
