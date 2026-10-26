//! The settings window content: application-level preferences (language,
//! appearance, session-database retention), applied live and persisted.
//! It lives as a modal window inside the viewer — the floating bar only
//! exposes capture-specific options.

use rust_i18n::t;

use crate::app::Shared;
use crate::config::{Appearance, DbRetention};
use crate::i18n::Language;
use crate::ui::{theme, widgets};

/// Draws the settings rows inside the viewer's modal settings window.
pub fn show(ui: &mut egui::Ui, shared: &Shared) {
    // Escape closes the modal like its title-bar button.
    if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        shared
            .settings_open
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
    let mut settings = shared.lock_settings();
    row(ui, &t!("settings.language.label"), |ui| {
        language_picker(ui, &mut settings);
    });
    widgets::hairline(ui);
    row(ui, &t!("settings.appearance.label"), |ui| {
        appearance_picker(ui, &mut settings);
    });
    widgets::hairline(ui);
    row(ui, &t!("settings.db_retention.label"), |ui| {
        retention_picker(ui, &mut settings, shared);
    });
    drop(settings);
    retention_confirm(ui.ctx(), shared);
}

/// One `label: [dropdown]` row, right-aligned control like the reference's
/// settings sheets. Every row allocates the SAME box (`row_h = 36`):
/// equal padding above and below the button, so the three rows between
/// the hairlines are identical in height and vertically centred inside
/// their slot.
fn row(ui: &mut egui::Ui, label: &str, control: impl FnOnce(&mut egui::Ui)) {
    let t = theme::tokens(ui.ctx());
    let row_h = 36.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::new(ui.available_width(), row_h),
        egui::Sense::hover(),
    );
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        egui::TextStyle::Body.resolve(ui.style()),
        t.text,
    );
    let mut right = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    control(&mut right);
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

/// egui-temp key holding a retention value waiting on the "captures
/// will be lost" confirmation.
const PENDING_RETENTION: &str = "pixme.pending_retention";

/// "Sauvegarde base de données" dropdown: `never` and `today` are
/// confirmed first when the store still holds captures — the choice
/// is parked in egui temp state until the dialog resolves.
fn retention_picker(ui: &mut egui::Ui, settings: &mut crate::config::Settings, shared: &Shared) {
    let old = settings.db_retention;
    let current = retention_label(old);
    let btn = widgets::dropdown_button(ui, &current);
    egui::Popup::menu(&btn).show(|ui| {
        for value in [DbRetention::Never, DbRetention::Today, DbRetention::Always] {
            ui.selectable_value(&mut settings.db_retention, value, retention_label(value));
        }
    });
    let new = settings.db_retention;
    if new == old {
        return;
    }
    if old.more_restrictive(new) && crate::app::lock(&shared.store).document_count() > 0 {
        // Park the choice, revert the shown value, let the dialog decide.
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new(PENDING_RETENTION), new));
        settings.db_retention = old;
    } else {
        apply_retention(new, settings, shared);
    }
}

/// Applies a confirmed retention choice: persists the setting and
/// prunes the store to match.
fn apply_retention(new: DbRetention, settings: &mut crate::config::Settings, shared: &Shared) {
    settings.db_retention = new;
    settings.save();
    crate::app::lock(&shared.store).apply_retention(new);
}

/// The pending-choice confirmation: applying loses stored captures —
/// Cancel keeps the previous setting untouched.
fn retention_confirm(ctx: &egui::Context, shared: &Shared) {
    let pending = ctx.data_mut(|d| d.get_temp::<DbRetention>(egui::Id::new(PENDING_RETENTION)));
    let Some(new) = pending else { return };
    let mut open = true;
    egui::Window::new(t!("settings.db.warn.title").to_string())
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(t!("settings.db.warn.body").to_string());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .button(t!("settings.db.warn.cancel").to_string())
                    .clicked()
                {
                    ctx.data_mut(|d| d.remove::<DbRetention>(egui::Id::new(PENDING_RETENTION)));
                }
                if ui
                    .button(t!("settings.db.warn.confirm").to_string())
                    .clicked()
                {
                    apply_retention(new, &mut shared.lock_settings(), shared);
                    ctx.data_mut(|d| d.remove::<DbRetention>(egui::Id::new(PENDING_RETENTION)));
                }
            });
        });
    if !open {
        ctx.data_mut(|d| d.remove::<DbRetention>(egui::Id::new(PENDING_RETENTION)));
    }
}

/// The translated label of one retention option.
fn retention_label(r: DbRetention) -> std::borrow::Cow<'static, str> {
    match r {
        DbRetention::Never => t!("settings.db.never"),
        DbRetention::Today => t!("settings.db.today"),
        DbRetention::Always => t!("settings.db.always"),
    }
}
