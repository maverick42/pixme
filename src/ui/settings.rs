//! The settings window content: application-level preferences (language,
//! appearance), applied live and persisted. It lives in its own viewport —
//! the floating bar only exposes capture-specific options.

use egui::{CentralPanel, Margin};
use rust_i18n::t;

use crate::app::Shared;
use crate::config::Appearance;
use crate::i18n::Language;
use crate::ui::widgets;

/// Draws the settings panel inside the settings viewport.
pub fn show(ui: &mut egui::Ui, shared: &Shared) {
    CentralPanel::default()
        .frame(egui::Frame::default().inner_margin(Margin::same(14)))
        .show(ui, |ui| {
            let mut settings = shared.lock_settings();
            row(ui, &t!("settings.language.label"), |ui| {
                language_picker(ui, &mut settings);
            });
            widgets::hairline(ui);
            row(ui, &t!("settings.appearance.label"), |ui| {
                appearance_picker(ui, &mut settings);
            });
        });
}

/// One `label: [dropdown]` row, right-aligned control like the reference's
/// settings sheets.
fn row(ui: &mut egui::Ui, label: &str, control: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
    });
    ui.add_space(6.0);
}

/// Language dropdown: `auto` follows the OS, explicit languages name
/// themselves. The central [`Shared::pump`] applies the locale and relabels
/// the OS menus on change.
fn language_picker(ui: &mut egui::Ui, settings: &mut crate::config::Settings) {
    let current = match settings.language {
        Language::Auto => t!("settings.language.auto"),
        explicit => std::borrow::Cow::Borrowed(explicit.native_name()),
    };
    let btn = widgets::dropdown_button(ui, &current);
    let mut changed = false;
    egui::Popup::menu(&btn).show(|ui| {
        changed |= ui
            .selectable_value(
                &mut settings.language,
                Language::Auto,
                t!("settings.language.auto"),
            )
            .changed();
        for lang in Language::ALL {
            changed |= ui
                .selectable_value(&mut settings.language, lang, lang.native_name())
                .changed();
        }
    });
    if changed {
        settings.save();
    }
}

/// Appearance dropdown: follow the OS, or pin the Pro / Pro Light palette.
fn appearance_picker(ui: &mut egui::Ui, settings: &mut crate::config::Settings) {
    let current = match settings.appearance {
        Appearance::Auto => t!("settings.appearance.auto"),
        Appearance::Dark => t!("settings.appearance.dark"),
        Appearance::Light => t!("settings.appearance.light"),
    };
    let btn = widgets::dropdown_button(ui, &current);
    let mut changed = false;
    egui::Popup::menu(&btn).show(|ui| {
        for (value, label) in [
            (Appearance::Auto, t!("settings.appearance.auto")),
            (Appearance::Dark, t!("settings.appearance.dark")),
            (Appearance::Light, t!("settings.appearance.light")),
        ] {
            changed |= ui
                .selectable_value(&mut settings.appearance, value, label)
                .changed();
        }
    });
    if changed {
        settings.save();
    }
}
