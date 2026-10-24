//! Single-line label that truncates with an ellipsis instead of wrapping.

use egui::{Response, Ui};

/// Draws `text` on one line, truncated to the available width.
pub fn elided_label(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.add(egui::Label::new(text.into()).truncate())
}
