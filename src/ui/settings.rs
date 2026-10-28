//! The settings window content, edited as a DRAFT: pages on the left
//! (General, File name), the page's rows on the right, and an
//! OK / Annuler footer. `OK` writes the draft back to `Shared.settings`
//! and persists it; Annuler / the painted cross / Escape discard it.
//! A retention switch that would lose stored captures asks for
//! confirmation on OK before anything is applied.

use std::sync::atomic::Ordering;

use rust_i18n::t;

use crate::app::{Shared, lock};
use crate::config::{Appearance, DbRetention, Settings};
use crate::i18n::Language;
use crate::ui::icons::Icon;
use crate::ui::{theme, widgets};

/// Which settings page the sidebar shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SettingsPage {
    /// Language, appearance and session-database retention.
    #[default]
    General,
    /// The saved-file filename template and its editable presets.
    Filename,
}

/// egui-temp keys: the active page, and a draft parked behind the
/// "captures will be lost" confirmation.
const PAGE_KEY: &str = "pixme.settings_page";
const PENDING_DRAFT: &str = "pixme.pending_settings_draft";

/// Draws the settings pages inside the viewer's modal settings window.
pub fn show(ui: &mut egui::Ui, shared: &Shared) {
    // Escape closes the modal like Annuler: the draft is dropped.
    if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        discard(shared);
    }
    let page = ui
        .ctx()
        .data_mut(|d| d.get_temp::<SettingsPage>(egui::Id::new(PAGE_KEY)))
        .unwrap_or_default();
    let mut page = page;
    {
        let mut guard = lock(&shared.settings_draft);
        let Some(draft) = guard.as_mut() else {
            return;
        };
        ui.horizontal(|ui| {
            sidebar(ui, &mut page);
            widgets::vline(ui, 140.0);
            ui.add_space(8.0);
            match page {
                SettingsPage::General => general_page(ui, draft),
                SettingsPage::Filename => filename_page(ui, draft),
            }
        });
    }
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new(PAGE_KEY), page));
    widgets::hairline(ui);
    ui.add_space(4.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        // An empty template could never produce a file name — OK stays
        // greyed until the field is usable again.
        let template_ok = lock(&shared.settings_draft)
            .as_ref()
            .is_some_and(|d| !d.filename_template.trim().is_empty());
        // right_to_left draws rightmost-first: OK first → visual order
        // [Annuler][OK], the primary rightmost like every other dialog.
        ui.add_enabled_ui(template_ok, |ui| {
            if widgets::primary_button(ui, &t!("draw.ok")).clicked() {
                apply(ui.ctx(), shared);
            }
        });
        if widgets::secondary_button(ui, &t!("draw.cancel")).clicked() {
            discard(shared);
        }
    });
    retention_confirm(ui.ctx(), shared);
}

/// The left page list: two entries for now, styled like the reference's
/// sheets — compact selectable rows, accent on the active one.
fn sidebar(ui: &mut egui::Ui, page: &mut SettingsPage) {
    ui.vertical(|ui| {
        ui.set_width(110.0);
        ui.add_space(4.0);
        for (value, key) in [
            (SettingsPage::General, "settings.page.general"),
            (SettingsPage::Filename, "settings.page.filename"),
        ] {
            if ui
                .selectable_label(*page == value, t!(key).to_string())
                .clicked()
            {
                *page = value;
            }
        }
    });
}

/// General page: the three preference rows that existed before.
fn general_page(ui: &mut egui::Ui, draft: &mut Settings) {
    ui.vertical(|ui| {
        ui.add_space(6.0);
        row(ui, &t!("settings.language.label"), |ui| {
            language_picker(ui, draft);
        });
        widgets::hairline(ui);
        row(ui, &t!("settings.appearance.label"), |ui| {
            appearance_picker(ui, draft);
        });
        widgets::hairline(ui);
        row(ui, &t!("settings.db_retention.label"), |ui| {
            retention_picker(ui, draft);
        });
    });
}

/// File-name page: editable template, preset dropdown plus +/− to edit
/// the preset list itself, the placeholder legend and a live example.
fn filename_page(ui: &mut egui::Ui, draft: &mut Settings) {
    let t = theme::tokens(ui.ctx());
    ui.vertical(|ui| {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(t!("settings.filename.label").to_string());
            let editor = egui::TextEdit::singleline(&mut draft.filename_template)
                .desired_width(220.0)
                .font(egui::TextStyle::Monospace);
            // Filename-hostile chars would break the save itself —
            // strip whatever slips through typing or paste.
            if ui.add(editor).changed() {
                draft
                    .filename_template
                    .retain(|c| !crate::config::filename::is_forbidden_filename_char(c));
            }
            let presets_btn = widgets::dropdown_button(ui, "");
            egui::Popup::menu(&presets_btn).show(|ui| {
                for preset in &draft.filename_templates {
                    if ui.button(egui::RichText::new(preset).monospace()).clicked() {
                        draft.filename_template = preset.clone();
                        ui.close();
                    }
                }
            });
            // Editable preset list without a third window: + stores the
            // field's content, − removes the preset matching it.
            if widgets::icon_button(ui, Icon::Plus, false)
                .on_hover_text(t!("settings.filename.add").to_string())
                .clicked()
            {
                let current = draft.filename_template.trim().to_owned();
                if !current.is_empty() && !draft.filename_templates.contains(&current) {
                    draft.filename_templates.push(current);
                }
            }
            if widgets::icon_button(ui, Icon::X, false)
                .on_hover_text(t!("settings.filename.remove").to_string())
                .clicked()
            {
                draft
                    .filename_templates
                    .retain(|p| p != &draft.filename_template);
            }
        });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(t!("settings.filename.notes").to_string())
                .monospace()
                .color(t.text_dim),
        );
        ui.add_space(8.0);
        let example = crate::config::expand_template(
            &draft.filename_template,
            draft.filename_counter,
            &chrono::Local::now(),
        );
        if draft.filename_template.trim().is_empty() {
            ui.colored_label(t.danger, t!("settings.filename.empty").to_string());
        } else {
            // `.png` already lives in the key — appending it here too
            // produced "… .png.png".
            ui.colored_label(
                t.text,
                t!("settings.filename.example", name = example.as_str()).to_string(),
            );
        }
    });
}

/// Writes the draft back and persists it — unless the new retention
/// would lose stored captures, which parks the draft behind the confirm
/// dialog first.
fn apply(ctx: &egui::Context, shared: &Shared) {
    let Some(draft) = lock(&shared.settings_draft).take() else {
        return;
    };
    let old_retention = lock(&shared.settings).db_retention;
    if old_retention.more_restrictive(draft.db_retention)
        && lock(&shared.store).document_count() > 0
    {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(PENDING_DRAFT), draft));
        return;
    }
    write_back(draft, shared);
}

/// The one write-back path: settings replaced + saved, retention applied
/// to the store, modal closed.
fn write_back(draft: Settings, shared: &Shared) {
    let retention = draft.db_retention;
    {
        let mut settings = lock(&shared.settings);
        *settings = draft;
        settings.save();
    }
    lock(&shared.store).apply_retention(retention);
    shared.settings_open.store(false, Ordering::Relaxed);
    *lock(&shared.settings_draft) = None;
}

/// Annuler / cross / Escape: the draft is dropped, nothing persists.
fn discard(shared: &Shared) {
    shared.settings_open.store(false, Ordering::Relaxed);
    *lock(&shared.settings_draft) = None;
}

/// The pending-draft confirmation: applying loses stored captures —
/// Cancel keeps the modal open with the previous retention value.
fn retention_confirm(ctx: &egui::Context, shared: &Shared) {
    let pending = ctx.data_mut(|d| d.get_temp::<Settings>(egui::Id::new(PENDING_DRAFT)));
    let Some(draft) = pending else { return };
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
                    // Revert only the retention choice; the rest of the
                    // draft stays for further editing.
                    let mut back = draft.clone();
                    back.db_retention = lock(&shared.settings).db_retention;
                    *lock(&shared.settings_draft) = Some(back);
                    ctx.data_mut(|d| d.remove::<Settings>(egui::Id::new(PENDING_DRAFT)));
                }
                if ui
                    .button(t!("settings.db.warn.confirm").to_string())
                    .clicked()
                {
                    ctx.data_mut(|d| d.remove::<Settings>(egui::Id::new(PENDING_DRAFT)));
                    write_back(draft.clone(), shared);
                }
            });
        });
    if !open {
        let mut back = draft.clone();
        back.db_retention = lock(&shared.settings).db_retention;
        *lock(&shared.settings_draft) = Some(back);
        ctx.data_mut(|d| d.remove::<Settings>(egui::Id::new(PENDING_DRAFT)));
    }
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
/// themselves. Persisted with the rest of the draft on OK; the central
/// [`Shared::pump`] then applies the locale and relabels the OS menus.
fn language_picker(ui: &mut egui::Ui, draft: &mut Settings) {
    let current = match draft.language {
        Language::Auto => t!("settings.language.auto"),
        explicit => std::borrow::Cow::Borrowed(explicit.native_name()),
    };
    let btn = widgets::dropdown_button(ui, &current);
    egui::Popup::menu(&btn).show(|ui| {
        ui.selectable_value(
            &mut draft.language,
            Language::Auto,
            t!("settings.language.auto"),
        );
        for lang in Language::ALL {
            ui.selectable_value(&mut draft.language, lang, lang.native_name());
        }
    });
}

/// Appearance dropdown: follow the OS, or pin the Pro / Pro Light palette.
fn appearance_picker(ui: &mut egui::Ui, draft: &mut Settings) {
    let current = match draft.appearance {
        Appearance::Auto => t!("settings.appearance.auto"),
        Appearance::Dark => t!("settings.appearance.dark"),
        Appearance::Light => t!("settings.appearance.light"),
    };
    let btn = widgets::dropdown_button(ui, &current);
    egui::Popup::menu(&btn).show(|ui| {
        for (value, label) in [
            (Appearance::Auto, t!("settings.appearance.auto")),
            (Appearance::Dark, t!("settings.appearance.dark")),
            (Appearance::Light, t!("settings.appearance.light")),
        ] {
            ui.selectable_value(&mut draft.appearance, value, label);
        }
    });
}

/// "Sauvegarde base de données" dropdown: the restrictive-case warning
/// fires at OK time (`apply`), not on selection — the draft only records
/// the choice.
fn retention_picker(ui: &mut egui::Ui, draft: &mut Settings) {
    let current = retention_label(draft.db_retention);
    let btn = widgets::dropdown_button(ui, &current);
    egui::Popup::menu(&btn).show(|ui| {
        for value in [DbRetention::Never, DbRetention::Today, DbRetention::Always] {
            ui.selectable_value(&mut draft.db_retention, value, retention_label(value));
        }
    });
}

/// The translated label of one retention option.
fn retention_label(r: DbRetention) -> std::borrow::Cow<'static, str> {
    match r {
        DbRetention::Never => t!("settings.db.never"),
        DbRetention::Today => t!("settings.db.today"),
        DbRetention::Always => t!("settings.db.always"),
    }
}
