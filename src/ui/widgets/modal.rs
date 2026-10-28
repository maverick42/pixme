//! Shared modal chrome — every in-app dialog shows the same header
//! band, colours and painted close cross (the Redimensionner look):
//! dock fill, card border, rounded corners, card-tinted title band
//! rounded on the top corners, strong title left and a red-hover
//! close cross right.

use egui::{Align, Color32, CornerRadius, Layout, Margin, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::ui::{
    icons::{self, Icon},
    theme,
};

/// The painted close cross — red soft fill + red X on hover (the
/// tab-close look). Returns `clicked`.
pub fn close_cross(ui: &mut Ui) -> bool {
    let t = theme::tokens(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        if hovered {
            let fill =
                Color32::from_rgba_unmultiplied(t.danger.r(), t.danger.g(), t.danger.b(), 34);
            ui.painter()
                .rect_filled(rect, CornerRadius::same(t.radius_sm), fill);
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(t.radius_sm),
                Stroke::new(1.0, t.danger),
                StrokeKind::Inside,
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        icons::paint(
            ui,
            rect,
            Icon::X,
            if hovered { 14.0 } else { 12.0 },
            if hovered { t.danger } else { t.icon },
        );
    }
    resp.clicked()
}

/// The standard in-app modal: `egui::Modal` with the app chrome —
/// dock fill + card border + `radius_lg`, the card-tinted title band
/// (strong title left, painted close cross right) and a 14 px body
/// margin. `inner.0` reports the painted cross click; `should_close()`
/// on the returned response still covers backdrop clicks and Escape.
pub fn modal_frame<T>(
    ctx: &egui::Context,
    id: egui::Id,
    title: &str,
    width: f32,
    add: impl FnOnce(&mut Ui) -> T,
) -> egui::ModalResponse<(bool, T)> {
    let t = theme::tokens(ctx);
    egui::Modal::new(id)
        .frame(
            egui::Frame::default()
                .fill(t.dock)
                .stroke(Stroke::new(1.0, t.card_border))
                .corner_radius(CornerRadius::same(t.radius_lg)),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            // Title band.
            let mut close_clicked = false;
            egui::Frame::default()
                .inner_margin(Margin::symmetric(12, 8))
                .fill(t.card)
                .corner_radius(CornerRadius {
                    nw: t.radius_lg,
                    ne: t.radius_lg,
                    sw: 0,
                    se: 0,
                })
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(title.to_owned()).color(t.text).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if close_cross(ui) {
                                close_clicked = true;
                            }
                        });
                    });
                });
            // Body.
            let inner = egui::Frame::default()
                .inner_margin(Margin::symmetric(14, 14))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 7.0;
                    add(ui)
                })
                .inner;
            (close_clicked, inner)
        })
}

/// The action row of a modal: right-aligned. Draw the PRIMARY button
/// FIRST so it lands at the far right, then secondaries to its left
/// — the visual order is `[secondary][primary]` with the blue button
/// rightmost, like on every other dialog.
pub fn modal_buttons<T>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> T) -> T {
    ui.with_layout(Layout::right_to_left(Align::Center), add)
        .inner
}
