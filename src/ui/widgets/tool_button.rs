//! Large toolbar button — icon above a text label with an optional
//! dropdown caret zone on the right edge (the functional reference's
//! editor band layout in the app's widget style).

use egui::{
    Align2, Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::ui::icons::{self, Icon};
use crate::ui::theme;

/// Button metrics (wider than `icon_button` — room for a caption).
pub const TOOL_W: f32 = 56.0;
/// Button height: icon slot + caption row.
pub const TOOL_H: f32 = 44.0;
/// Right-edge zone that opens the menu instead of the main action.
const CARET_W: f32 = 13.0;
/// Icon side inside the button.
const ICON_SIZE: f32 = 18.0;

/// One large toolbar button. When `has_menu` is set, the right caret zone
/// is drawn and `clicked()` hits inside it are reported via
/// [`caret_hit`]. `enabled` greys the whole button and swallows clicks.
pub fn tool_button(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    has_menu: bool,
    selected: bool,
    enabled: bool,
) -> Response {
    let t = theme::tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(TOOL_W, TOOL_H), Sense::click());
    let (icon_tint, text_col) = if enabled {
        (t.icon, t.text)
    } else {
        (t.text_faint, t.text_faint)
    };
    if ui.is_rect_visible(rect) {
        let (fill, stroke) = if !enabled {
            (Color32::TRANSPARENT, Stroke::NONE)
        } else if response.is_pointer_button_down_on() {
            (t.pressed, Stroke::NONE)
        } else if selected {
            (t.accent_soft, Stroke::new(1.0, t.accent_border))
        } else if response.hovered() {
            (t.hover, Stroke::NONE)
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
        let main_w = if has_menu {
            rect.width() - CARET_W
        } else {
            rect.width()
        };
        let main = Rect::from_min_size(rect.min, Vec2::new(main_w, rect.height()));
        icons::paint(
            ui,
            Rect::from_center_size(
                Pos2::new(main.center().x, main.min.y + 15.0),
                Vec2::splat(ICON_SIZE),
            ),
            icon,
            ICON_SIZE,
            icon_tint,
        );
        // Caption: centred in the main zone, clipped to it.
        let label_rect = Rect::from_min_size(
            Pos2::new(main.min.x, main.max.y - 15.0),
            Vec2::new(main_w, 13.0),
        );
        ui.painter().with_clip_rect(label_rect).text(
            label_rect.center(),
            Align2::CENTER_CENTER,
            label,
            theme::medium(10.0),
            text_col,
        );
        if has_menu {
            icons::paint(
                ui,
                Rect::from_center_size(
                    Pos2::new(rect.max.x - CARET_W * 0.5 - 1.0, main.min.y + 15.0),
                    Vec2::splat(9.0),
                ),
                Icon::ChevronDown,
                9.0,
                icon_tint,
            );
        }
    }
    response
}

/// Whether the click landed in the caret zone (`has_menu` buttons only):
/// caret = open the popup, otherwise the main action.
pub fn caret_hit(response: &Response, has_menu: bool) -> bool {
    has_menu
        && response
            .interact_pointer_pos()
            .is_some_and(|pos| pos.x > response.rect.max.x - CARET_W)
}
