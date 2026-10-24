//! Everything the user sees: theme (single source of visual truth), icons,
//! reusable widgets and the floating toolbar.
//!
//! Rule: no hard-coded colour, radius, spacing or font outside `theme.rs` and
//! `widgets/` — everything reads [`theme::tokens`].

pub mod icons;
pub mod settings;
pub mod theme;
pub mod toolbar;
pub mod viewer;
pub mod widgets;

mod icon_data;

/// One-time egui setup: custom fonts and the image (SVG) loaders.
pub fn install(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    theme::install_fonts(ctx);
}
