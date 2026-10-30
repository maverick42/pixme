//! Rounded panel/card frame (adapted from the visual reference; MIT OR
//! Apache-2.0 — see ATTRIBUTION.md).

use egui::{CornerRadius, Frame, Margin, Shadow, Stroke};

use crate::ui::theme::{self, Tokens};

/// A `Frame` filled with `card`, stroked with `card_border`, radius `lg`.
pub fn card_frame(t: Tokens) -> Frame {
    Frame::default()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.card_border))
        .corner_radius(CornerRadius::same(t.radius_lg))
        .inner_margin(Margin::same(6))
}

/// The soft window/panel shadow used under floating surfaces.
pub fn card_shadow(t: Tokens) -> Shadow {
    Shadow {
        offset: [0, 6],
        blur: 16,
        spread: 0,
        color: t.shadow,
    }
}

/// Reads the active tokens, then [`card_frame`].
pub fn card(ui: &egui::Ui) -> Frame {
    card_frame(theme::tokens(ui.ctx()))
}
