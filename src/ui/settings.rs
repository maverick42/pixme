//! The settings window content, edited as a DRAFT: pages on the left
//! (General, File name, About), the page's rows on the right, and an
//! OK / Annuler footer. `OK` writes the draft back to `Shared.settings`
//! and persists it; Annuler / the painted cross / Escape discard it.
//! A retention switch that would lose stored captures asks for
//! confirmation on OK before anything is applied.

use std::sync::atomic::Ordering;

use rust_i18n::t;

use crate::app::{Shared, lock};
use crate::config::{Appearance, DbRetention, IconStyle, Settings};
use crate::i18n::Language;
use crate::ui::icons::Icon;
use crate::ui::{theme, widgets};

/// Which settings page the sidebar shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPage {
    /// Language, appearance and session-database retention.
    #[default]
    General,
    /// The saved-file filename template and its editable presets.
    Filename,
    /// OS-wide shortcuts (Open PixMe, region capture).
    Hotkeys,
    /// App identity, version, data/log folder locations, diagnostics.
    About,
}

/// egui-temp keys: the active page, and a draft parked behind the
/// "captures will be lost" confirmation.
const PAGE_KEY: &str = "pixme.settings_page";
const PENDING_DRAFT: &str = "pixme.pending_settings_draft";

/// Draws the settings pages inside the viewer's modal settings window.
pub fn show(ui: &mut egui::Ui, shared: &Shared) {
    // Escape closes the modal like Annuler: the draft is dropped.
    let recording_active = ui
        .ctx()
        .data_mut(|d| d.get_temp::<Option<crate::hotkeys::HotkeyAction>>(egui::Id::new(REC_KEY)))
        .is_some_and(|a| a.is_some());
    if !recording_active && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        discard(ui.ctx(), shared);
    }
    // A pending retention confirm replaces the whole body — a separate
    // egui::Window on the same ctx gets its clicks eaten by the modal.
    if retention_confirm(ui, shared) {
        return;
    }
    let page = ui
        .ctx()
        .data_mut(|d| d.get_temp::<SettingsPage>(egui::Id::new(PAGE_KEY)))
        .unwrap_or_default();
    let mut page = page;
    // Leaving the Hotkeys page while a field listens (sidebar switch)
    // would leave the global hotkeys suspended forever — clear the
    // recorder and re-arm.
    if page != SettingsPage::Hotkeys {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new(REC_KEY), None::<crate::hotkeys::HotkeyAction>)
        });
        // A test pending off-page never resolves (the page owns the
        // timeout): cancel it — the draft is untouched — and release
        // any transient registration.
        #[cfg(target_os = "macos")]
        {
            type Testing = (crate::hotkeys::HotkeyAction, u32, std::time::Instant);
            if ui
                .ctx()
                .data_mut(|d| {
                    d.get_temp::<Option<Testing>>(egui::Id::new(TEST_KEY))
                        .flatten()
                })
                .is_some()
            {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new(TEST_KEY), None::<Testing>));
                *lock(&shared.hotkey_verify) = None;
            }
        }
        if let Some(m) = lock(&shared.hotkeys).as_mut() {
            m.resume();
            m.release_verify();
        }
    }
    {
        let mut guard = lock(&shared.settings_draft);
        let Some(draft) = guard.as_mut() else {
            return;
        };
        ui.horizontal(|ui| {
            sidebar(ui, &mut page);
            widgets::vline(ui, 140.0);
            ui.add_space(8.0);
            // Constant 140 px content box: keeps the Annuler/OK row at the
            // exact same vertical position across all three pages without
            // layout feedback loops.
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 140.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| match page {
                    SettingsPage::General => general_page(ui, draft),
                    SettingsPage::Filename => filename_page(ui, draft),
                    SettingsPage::Hotkeys => hotkeys_page(ui, draft, shared),
                    SettingsPage::About => about_page(ui),
                },
            );
        });
    }
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new(PAGE_KEY), page));
    // Footer pinned to the window's bottom edge: the remaining space
    // above is consumed first, so the row never moves between pages.
    ui.add_space((ui.available_height() - 28.0).max(0.0));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        // An empty template could never produce a file name — OK stays
        // greyed until the field is usable again.
        let template_ok = lock(&shared.settings_draft)
            .as_ref()
            .is_some_and(|d| !d.filename_template.trim().is_empty());
        // A hotkey recording or a pending confirmation press must
        // resolve first — applying mid-verification would arm a combo
        // that may be silently swallowed by another app, and
        // mid-recording would persist while the armed set is
        // suspended.
        let ready = template_ok && !hotkey_pending(ui.ctx());
        // right_to_left draws rightmost-first: OK first → visual order
        // [Annuler][OK], the primary rightmost like every other dialog.
        ui.add_enabled_ui(ready, |ui| {
            if widgets::primary_button(ui, &t!("draw.ok")).clicked() {
                apply(ui.ctx(), shared);
            }
        });
        if widgets::secondary_button(ui, &t!("draw.cancel")).clicked() {
            discard(ui.ctx(), shared);
        }
    });
}

/// The left page list: two entries for now, styled like the reference's
/// sheets — compact selectable rows, accent on the active one.
fn sidebar(ui: &mut egui::Ui, page: &mut SettingsPage) {
    ui.vertical(|ui| {
        ui.set_width(110.0);
        ui.add_space(4.0);
        // A few pixels of air between the page options.
        ui.spacing_mut().item_spacing.y = 4.0;
        for (value, key) in [
            (SettingsPage::General, "settings.page.general"),
            (SettingsPage::Filename, "settings.page.filename"),
            (SettingsPage::Hotkeys, "settings.page.hotkeys"),
            (SettingsPage::About, "settings.page.about"),
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

/// General page: the persisted preference rows, plus the session-only
/// diagnostic-mode switch (applied live — it is not part of the draft).
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
        row(ui, &t!("settings.icon_style.label"), |ui| {
            icon_style_picker(ui, draft);
        });
        widgets::hairline(ui);
        row(ui, &t!("settings.db_retention.label"), |ui| {
            retention_picker(ui, draft);
        });
        widgets::hairline(ui);
        row(ui, &t!("settings.db_status.label"), |ui| {
            ui.checkbox(&mut draft.db_status, "")
                .on_hover_text(t!("settings.db_status.tooltip").to_string());
        });
        widgets::hairline(ui);
        row(ui, &t!("settings.general.diagnostic"), |ui| {
            let mut on = crate::logging::diagnostic_enabled();
            if ui
                .checkbox(&mut on, "")
                .on_hover_text(t!("settings.general.diagnostic.tooltip").to_string())
                .changed()
            {
                crate::logging::set_diagnostic(on);
            }
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

/// "Dossiers" page: the data and log folder locations — copyable,
/// openable in the OS file manager — plus the diagnostics bundle.
fn about_page(ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    ui.vertical(|ui| {
        ui.add_space(4.0);
        // App identity — the content the standalone About window used to
        // show: the 256 artwork, the name, the running version, licence.
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let icon_rgba = crate::ui::icons::app_icon_rgba(128);
            let icon_img = egui::ColorImage::from_rgba_unmultiplied([128, 128], &icon_rgba);
            let icon_tex =
                ui.ctx()
                    .load_texture("pixme://about-icon", icon_img, egui::TextureOptions::LINEAR);
            ui.image(egui::load::SizedTexture::new(
                icon_tex.id(),
                egui::vec2(48.0, 48.0),
            ));
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("PixMe").color(t.text).strong());
                ui.label(
                    egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .size(10.5)
                        .color(t.text_dim),
                );
                ui.label(
                    egui::RichText::new(t!("settings.about.license").to_string())
                        .size(10.5)
                        .color(t.text_faint),
                );
            });
        });
        ui.add_space(8.0);
        // Label column sized to the longest row label, so both paths
        // start at the same x whatever the locale's word lengths.
        let label_w = [
            t!("settings.about.data_folder").to_string(),
            t!("settings.about.logs_folder").to_string(),
        ]
        .iter()
        .map(|s| {
            ui.painter()
                .layout_no_wrap(
                    s.clone(),
                    egui::FontId::new(10.5, egui::FontFamily::Proportional),
                    t.text_dim,
                )
                .size()
                .x
        })
        .fold(0.0, f32::max)
        .ceil();
        folder_row(
            ui,
            &t!("settings.about.data_folder"),
            crate::paths::data_dir(),
            "settings.about.open_data",
            label_w,
        );
        folder_row(
            ui,
            &t!("settings.about.logs_folder"),
            crate::paths::logs_dir(),
            "settings.about.open_logs",
            label_w,
        );
        ui.add_space(4.0);
        if widgets::secondary_button(ui, &t!("settings.about.copy_diagnostics")).clicked() {
            ui.ctx().copy_text(crate::logging::diagnostics_text());
            tracing::info!("diagnostics_copied");
        }
        ui.label(
            egui::RichText::new(t!("settings.about.diagnostics_hint").to_string())
                .size(10.5)
                .color(t.text_faint),
        );
    });
}

/// Opens the settings modal straight on `page` — the About menu entry
/// uses it; `show` reads the choice back through `PAGE_KEY`.
pub fn open_at(ctx: &egui::Context, page: SettingsPage) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(PAGE_KEY), page));
}

/// One About-page location row: label, the path (elided, full path on
/// hover), a copy button and an open-in-file-manager button.
fn folder_row(
    ui: &mut egui::Ui,
    label: &str,
    path: &std::path::Path,
    open_key: &'static str,
    label_w: f32,
) {
    let t = theme::tokens(ui.ctx());
    let path_text = path.display().to_string();
    ui.horizontal(|ui| {
        // `add_sized` would centre the text in its box (`centered_and_justified`)
        // — a shorter label or path would then start right of a longer row's.
        ui.allocate_ui_with_layout(
            egui::vec2(label_w + 2.0, 16.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .size(10.5)
                        .color(t.text_dim)
                        .strong(),
                );
            },
        );
        // Fixed-width truncating path (full path on hover): the icons
        // stay at a fixed right position on both rows and long paths
        // never stretch the window. Left-aligned so both rows' paths
        // start at the same x.
        ui.allocate_ui_with_layout(
            egui::vec2(220.0, 16.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(&path_text).size(10.5).color(t.text))
                        .truncate(),
                )
                .on_hover_text(&path_text);
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::icon_button(ui, Icon::FolderOpen, false)
                .on_hover_text(t!(open_key).to_string())
                .clicked()
            {
                tracing::info!(folder = %crate::paths::file_name(path), "folder_opened");
                crate::platform::open_folder(path);
            }
            if widgets::icon_button(ui, Icon::Copy, false)
                .on_hover_text(t!("settings.about.copy_path").to_string())
                .clicked()
            {
                ui.ctx().copy_text(path_text.clone());
            }
        });
    });
}

/// Writes the draft back and persists it — unless the new retention
/// would lose stored captures, which parks the draft behind the confirm
/// dialog first.
fn apply(ctx: &egui::Context, shared: &Shared) {
    let Some(draft) = lock(&shared.settings_draft).clone() else {
        return;
    };
    let old_retention = lock(&shared.settings).db_retention;
    if old_retention.more_restrictive(draft.db_retention) && shared.writer.document_count() > 0 {
        // Park a COPY: the live draft stays in the modal so the
        // settings window keeps rendering behind the confirm.
        ctx.data_mut(|d| d.insert_persisted(egui::Id::new(PENDING_DRAFT), draft));
        return;
    }
    write_back(draft, shared);
}

/// One `settings_changed` event per modified preference — the setting
/// name and its new value; counters, recents and last-used dirs are
/// deliberately left out (they churn silently on every save).
fn log_changes(old: &Settings, new: &Settings) {
    type Field = (&'static str, fn(&Settings) -> String);
    let fields: [Field; 9] = [
        ("language", |s| format!("{:?}", s.language)),
        ("appearance", |s| format!("{:?}", s.appearance)),
        ("icon_style", |s| format!("{:?}", s.icon_style)),
        ("copy_to_clipboard", |s| s.copy_to_clipboard.to_string()),
        ("db_retention", |s| format!("{:?}", s.db_retention)),
        ("db_status", |s| s.db_status.to_string()),
        ("tab_sort", |s| format!("{:?}", s.tab_sort)),
        ("draw_opts", |s| {
            format!("{:?}", s.draw_opts.tools.keys().count())
        }),
        ("filename_template", |s| s.filename_template.clone()),
    ];
    for (name, f) in fields {
        let (a, b) = (f(old), f(new));
        if a != b {
            tracing::info!(setting = name, value = %b, "settings_changed");
        }
    }
}

/// The one write-back path: settings replaced + saved, retention applied
/// to the store, modal closed.
fn write_back(draft: Settings, shared: &Shared) {
    let retention = draft.db_retention;
    let was = {
        let mut settings = lock(&shared.settings);
        let was = settings.db_retention;
        let old = std::mem::replace(&mut *settings, draft);
        log_changes(&old, &settings);
        settings.save();
        was
    };
    shared.writer.retention(retention);
    // The apply may have pruned (or emptied) rows the live tabs still
    // believe persisted — or persistence may just have come back on
    // after `Never`: forget the write baseline so the next drain
    // re-queues everything the store is now missing. Done through a
    // flag: `write_back` runs inside the settings modal, which the
    // viewer pass paints while HOLDING the viewer lock — locking the
    // viewer here would self-deadlock. `sync_store` consumes the flag.
    use crate::config::DbRetention as R;
    if was != R::Always || retention != R::Always {
        shared
            .store_marks_reset
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    shared.settings_open.store(false, Ordering::Relaxed);
    *lock(&shared.settings_draft) = None;
    *lock(&shared.hotkey_verify) = None;
    // A field may have been listening — re-arm whatever was suspended
    // and release any pending verification registration.
    if let Some(m) = lock(&shared.hotkeys).as_mut() {
        m.resume();
        m.release_verify();
    }
    // Re-arm the global hotkeys against the just-persisted map.
    shared.push(crate::app::Command::HotkeysChanged);
}

/// `true` while a hotkey field is recording or a recorded combo waits
/// for its confirmation press — the footer OK stays disabled until the
/// binding resolves (confirmed, rejected or cancelled).
fn hotkey_pending(ctx: &egui::Context) -> bool {
    let recording = ctx.data_mut(|d| {
        d.get_temp::<Option<crate::hotkeys::HotkeyAction>>(egui::Id::new(REC_KEY))
            .is_some_and(|a| a.is_some())
    });
    if recording {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        ctx.data_mut(|d| {
            d.get_temp::<Option<(crate::hotkeys::HotkeyAction, u32, std::time::Instant)>>(
                egui::Id::new(TEST_KEY),
            )
            .is_some_and(|v| v.is_some())
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Drops the hotkey recorder's egui-temp state — every modal close
/// path calls it: a stale `Some` would reopen settings already
/// "listening" (OK greyed, armed combos still firing their actions).
pub fn end_recording(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(REC_KEY), None::<crate::hotkeys::HotkeyAction>));
}

/// Annuler / cross / Escape: the draft is dropped, nothing persists.
fn discard(ctx: &egui::Context, shared: &Shared) {
    shared.settings_open.store(false, Ordering::Relaxed);
    *lock(&shared.settings_draft) = None;
    *lock(&shared.hotkey_verify) = None;
    end_recording(ctx);
    // A field may have been listening — re-arm whatever was suspended
    // and release any pending verification registration.
    if let Some(m) = lock(&shared.hotkeys).as_mut() {
        m.resume();
        m.release_verify();
    }
}

/// The pending-draft confirmation: applying loses stored captures —
/// Cancel keeps the modal open with the previous retention value.
/// Drawn INSIDE the settings window (returns `true` while active): a
/// separate egui::Window gets its clicks eaten by the modal.
fn retention_confirm(ui: &mut egui::Ui, shared: &Shared) -> bool {
    let ctx = ui.ctx().clone();
    let pending = ctx.data_mut(|d| d.get_persisted::<Settings>(egui::Id::new(PENDING_DRAFT)));
    let Some(draft) = pending else {
        return false;
    };
    let t = theme::tokens(&ctx);
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(t!("settings.db.warn.title").to_string())
                .color(t.text)
                .size(13.0)
                .strong(),
        );
        ui.add_space(8.0);
        ui.label(egui::RichText::new(t!("settings.db.warn.body").to_string()).color(t.text_dim));
        ui.add_space(16.0);
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
    true
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

/// Capture-bar icon-style dropdown: Lucide monochrome (the reference
/// look), the same glyphs in per-icon colours, or Phosphor two-tones.
fn icon_style_picker(ui: &mut egui::Ui, draft: &mut Settings) {
    let label = |s: IconStyle| match s {
        IconStyle::Standard => t!("settings.icon_style.standard"),
        IconStyle::Color => t!("settings.icon_style.color"),
        IconStyle::Duotone => t!("settings.icon_style.duotone"),
    };
    let current = label(draft.icon_style);
    let btn = widgets::dropdown_button(ui, &current);
    egui::Popup::menu(&btn).show(|ui| {
        for value in [IconStyle::Standard, IconStyle::Color, IconStyle::Duotone] {
            ui.selectable_value(&mut draft.icon_style, value, label(value));
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

/// egui-temp keys: which action's field is listening, record-time
/// errors (keyed by action), and the macOS confirmation state.
const REC_KEY: &str = "pixme.hk_recording";
const REC_ERR: &str = "pixme.hk_rec_err";
#[cfg(target_os = "macos")]
const TEST_KEY: &str = "pixme.hk_test";
/// How long the test window stays open — the press is synthesized by
/// the app, so it either lands within a frame or never does; 2 s is
/// generous headroom for the Carbon dispatch round-trip.
#[cfg(target_os = "macos")]
const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

fn action_label_key(a: crate::hotkeys::HotkeyAction) -> &'static str {
    match a {
        crate::hotkeys::HotkeyAction::OpenViewer => "settings.hotkeys.open_viewer",
        crate::hotkeys::HotkeyAction::CaptureRegion => "settings.hotkeys.capture_region",
    }
}

/// The persisted accelerator ("control+shift+KeyR") rendered for the
/// user: Ctrl/Alt/Shift + Cmd (macOS) / Win, then the bare key name.
fn combo_label(raw: &str) -> String {
    use global_hotkey::hotkey::{HotKey, Modifiers};
    let Ok(hk) = raw.parse::<HotKey>() else {
        return raw.to_owned();
    };
    let mut parts: Vec<String> = Vec::new();
    if hk.mods.contains(Modifiers::CONTROL) {
        parts.push("Ctrl".into());
    }
    if hk.mods.contains(Modifiers::ALT) {
        parts.push(
            if cfg!(target_os = "macos") {
                "Opt"
            } else {
                "Alt"
            }
            .into(),
        );
    }
    if hk.mods.contains(Modifiers::SHIFT) {
        parts.push("Shift".into());
    }
    if hk.mods.contains(Modifiers::META) {
        parts.push(
            if cfg!(target_os = "macos") {
                "Cmd"
            } else {
                "Win"
            }
            .into(),
        );
    }
    let key = hk.key.to_string();
    parts.push(
        key.strip_prefix("Key")
            .or_else(|| key.strip_prefix("Digit"))
            .unwrap_or(&key)
            .to_owned(),
    );
    parts.join("+")
}

/// `egui::Key` → `keyboard-types::Code` for the combos worth
/// recording. `None` = key not bindable (kept listening, error shown).
fn egui_to_code(key: egui::Key) -> Option<global_hotkey::hotkey::Code> {
    use egui::Key as K;
    use global_hotkey::hotkey::Code;
    Some(match key {
        K::A => Code::KeyA,
        K::B => Code::KeyB,
        K::C => Code::KeyC,
        K::D => Code::KeyD,
        K::E => Code::KeyE,
        K::F => Code::KeyF,
        K::G => Code::KeyG,
        K::H => Code::KeyH,
        K::I => Code::KeyI,
        K::J => Code::KeyJ,
        K::K => Code::KeyK,
        K::L => Code::KeyL,
        K::M => Code::KeyM,
        K::N => Code::KeyN,
        K::O => Code::KeyO,
        K::P => Code::KeyP,
        K::Q => Code::KeyQ,
        K::R => Code::KeyR,
        K::S => Code::KeyS,
        K::T => Code::KeyT,
        K::U => Code::KeyU,
        K::V => Code::KeyV,
        K::W => Code::KeyW,
        K::X => Code::KeyX,
        K::Y => Code::KeyY,
        K::Z => Code::KeyZ,
        K::Num0 => Code::Digit0,
        K::Num1 => Code::Digit1,
        K::Num2 => Code::Digit2,
        K::Num3 => Code::Digit3,
        K::Num4 => Code::Digit4,
        K::Num5 => Code::Digit5,
        K::Num6 => Code::Digit6,
        K::Num7 => Code::Digit7,
        K::Num8 => Code::Digit8,
        K::Num9 => Code::Digit9,
        K::F1 => Code::F1,
        K::F2 => Code::F2,
        K::F3 => Code::F3,
        K::F4 => Code::F4,
        K::F5 => Code::F5,
        K::F6 => Code::F6,
        K::F7 => Code::F7,
        K::F8 => Code::F8,
        K::F9 => Code::F9,
        K::F10 => Code::F10,
        K::F11 => Code::F11,
        K::F12 => Code::F12,
        K::ArrowUp => Code::ArrowUp,
        K::ArrowDown => Code::ArrowDown,
        K::ArrowLeft => Code::ArrowLeft,
        K::ArrowRight => Code::ArrowRight,
        K::Space => Code::Space,
        K::Tab => Code::Tab,
        K::Backspace => Code::Backspace,
        K::Delete => Code::Delete,
        K::Insert => Code::Insert,
        K::Home => Code::Home,
        K::End => Code::End,
        K::PageUp => Code::PageUp,
        K::PageDown => Code::PageDown,
        K::Minus => Code::Minus,
        K::Comma => Code::Comma,
        K::Period => Code::Period,
        K::Slash => Code::Slash,
        K::Backslash => Code::Backslash,
        K::Semicolon => Code::Semicolon,
        K::OpenBracket => Code::BracketLeft,
        K::CloseBracket => Code::BracketRight,
        K::Quote => Code::Quote,
        K::Backtick => Code::Backquote,
        K::Enter => Code::Enter,
        _ => return None,
    })
}

/// Hotkeys page: one row per bindable action — translated label, a
/// field-style record panel (bordered, accent while listening), ✕ to
/// unset — plus a per-row error badge and a restore-defaults button
/// (defaults = no binding). Modifier-only presses and unmapped keys
/// are ignored while listening; only a validated combo or Escape ends
/// the recording. A replacement is validated (dup / reserved / OS
/// probe) before the draft is touched; the armed binding is only
/// swapped after the OS accepted the new one.
fn hotkeys_page(ui: &mut egui::Ui, draft: &mut Settings, shared: &Shared) {
    use crate::hotkeys::HotkeyAction;
    let ctx = ui.ctx().clone();
    let tk = theme::tokens(&ctx);
    let mut recording: Option<HotkeyAction> = ctx
        .data_mut(|d| d.get_temp::<Option<HotkeyAction>>(egui::Id::new(REC_KEY)))
        .flatten();
    // A recording that just ended (success, Escape) re-arms the
    // suspended hotkeys — the frame after the recording state cleared.
    if recording.is_none()
        && lock(&shared.hotkeys)
            .as_ref()
            .is_some_and(|m| m.is_suspended())
    {
        lock(&shared.hotkeys)
            .as_mut()
            .expect("checked above")
            .resume();
    }
    let mut errors: std::collections::BTreeMap<HotkeyAction, String> = ctx
        .data_mut(|d| d.get_temp(egui::Id::new(REC_ERR)))
        .unwrap_or_default();
    // macOS: the "Test" button synthesizes one real press of the
    // bound combo and arms a short listen window — the listener
    // consumes the press and clears the shared slot; a timeout means
    // the combo is swallowed by another app (Carbon cannot detect
    // foreign registrations at registration time). No-op on Windows —
    // `RegisterHotKey` fails there when the combo is taken, so a test
    // press is pointless.
    #[cfg(target_os = "macos")]
    let mut testing: Option<(HotkeyAction, u32, std::time::Instant)> = ctx.data_mut(|d| {
        d.get_temp::<Option<(HotkeyAction, u32, std::time::Instant)>>(egui::Id::new(TEST_KEY))
            .flatten()
    });
    #[cfg(not(target_os = "macos"))]
    let testing: Option<(HotkeyAction, u32, std::time::Instant)> = None;

    // Capture the next real key while a field is listening.
    if let Some(action) = recording {
        let events: Vec<egui::Event> = ctx.input(|i| i.events.to_vec());
        for ev in events {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = ev
            else {
                continue;
            };
            if key == egui::Key::Escape {
                recording = None;
                break;
            }
            let mut mods = global_hotkey::hotkey::Modifiers::empty();
            if modifiers.ctrl {
                mods |= global_hotkey::hotkey::Modifiers::CONTROL;
            }
            if modifiers.alt {
                mods |= global_hotkey::hotkey::Modifiers::ALT;
            }
            if modifiers.shift {
                mods |= global_hotkey::hotkey::Modifiers::SHIFT;
            }
            if modifiers.mac_cmd {
                mods |= global_hotkey::hotkey::Modifiers::META;
            }
            // macOS can deliver the key event with an empty modifiers
            // field (the frame state is authoritative) — fall back to
            // the frame's modifier state before giving up.
            if mods.is_empty() {
                let f = ctx.input(|i| i.modifiers);
                if f.ctrl {
                    mods |= global_hotkey::hotkey::Modifiers::CONTROL;
                }
                if f.alt {
                    mods |= global_hotkey::hotkey::Modifiers::ALT;
                }
                if f.shift {
                    mods |= global_hotkey::hotkey::Modifiers::SHIFT;
                }
                if f.mac_cmd {
                    mods |= global_hotkey::hotkey::Modifiers::META;
                }
            }
            tracing::debug!(?key, ?mods, "hotkey_record_key");
            if mods.is_empty() {
                continue; // modifier key alone — keep listening
            }
            let Some(code) = egui_to_code(key) else {
                continue; // unmapped key — keep listening
            };
            let hk = global_hotkey::hotkey::HotKey::new(Some(mods), code);
            let raw = hk.to_string();
            let dup = draft.hotkeys.iter().any(|(a, s)| *a != action && *s == raw);
            // A combo already armed — for this or the other action —
            // is proven deliverable: skip the probe (it would report
            // it taken by our own registration) and the confirmation
            // press (a transient register would collide with the live
            // one — the lib keeps a single ref per id).
            let already_armed = lock(&shared.hotkeys)
                .as_ref()
                .is_some_and(|m| m.is_armed(&hk));
            let err = if already_armed {
                None
            } else if dup {
                Some(t!("settings.hotkeys.err_dup").to_string())
            } else {
                // `probe` reuses the app's single manager — on macOS a
                // second Carbon event handler cannot be installed, so
                // a throwaway manager would fail every probe.
                let probe = lock(&shared.hotkeys)
                    .as_ref()
                    .ok_or(crate::hotkeys::HotkeyError::Taken)
                    .and_then(|m| m.probe(&raw));
                match probe {
                    Ok(()) => None,
                    Err(crate::hotkeys::HotkeyError::Reserved) => {
                        Some(t!("settings.hotkeys.err_reserved").to_string())
                    }
                    Err(crate::hotkeys::HotkeyError::Taken) => {
                        Some(t!("settings.hotkeys.err_taken").to_string())
                    }
                }
            };
            if let Some(e) = err {
                errors.insert(action, e);
            } else {
                draft.hotkeys.insert(action, raw);
                errors.remove(&action);
            }
            recording = None;
            break;
        }
    }

    // macOS: watch the test window — the press proves the combo is
    // deliverable (listener consumed it and cleared the slot); a
    // timeout means another app swallows it — surface both as toasts.
    #[cfg(target_os = "macos")]
    if let Some((_, id, at)) = testing {
        if *lock(&shared.hotkey_verify) != Some(id) {
            testing = None; // the test press arrived
            widgets::toast(
                &ctx,
                widgets::ToastKind::Success,
                t!("settings.hotkeys.test_ok").to_string(),
            );
            if let Some(m) = lock(&shared.hotkeys).as_mut() {
                m.release_verify();
            }
        } else if at.elapsed() > TEST_TIMEOUT {
            testing = None;
            widgets::toast(
                &ctx,
                widgets::ToastKind::Warning,
                t!("settings.hotkeys.test_fail").to_string(),
            );
            *lock(&shared.hotkey_verify) = None;
            if let Some(m) = lock(&shared.hotkeys).as_mut() {
                m.release_verify();
            }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    ui.vertical(|ui| {
        ui.add_space(6.0);
        let sync_errors = lock(&shared.hotkey_errors).clone();
        for action in HotkeyAction::ALL {
            ui.horizontal(|ui| {
                ui.label(t!(action_label_key(action)).to_string());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let bound = draft.hotkeys.get(&action).is_some_and(|s| !s.is_empty());
                    let testing_action = testing.is_some_and(|(a, _, _)| a == action);
                    // "Test" synthesizes one real press of the bound
                    // combo — whoever owns it receives it; if it is
                    // ours, the listener consumes it (no action fires)
                    // and a toast confirms. The button stays visible,
                    // disabled while the window runs and while a
                    // recording is in progress (armed combos are
                    // suspended then — the test could not fire).
                    #[cfg(target_os = "macos")]
                    if bound {
                        ui.add_enabled_ui(!testing_action && recording.is_none(), |ui| {
                            let test_resp =
                                widgets::secondary_button(ui, &t!("settings.hotkeys.test"));
                            if test_resp.clicked()
                                && let Some(raw) = draft.hotkeys.get(&action)
                                && let Ok(hk) = raw.parse::<global_hotkey::hotkey::HotKey>()
                            {
                                *lock(&shared.hotkey_verify) = Some(hk.id);
                                testing = Some((action, hk.id, std::time::Instant::now()));
                                if let Some(m) = lock(&shared.hotkeys).as_mut() {
                                    m.release_verify();
                                    if !m.is_armed(&hk) {
                                        m.register_verify(hk, action);
                                    }
                                }
                                crate::hotkeys::synthesize(&hk);
                            }
                        });
                    }
                    if bound
                        && widgets::icon_button(ui, Icon::X, false)
                            .on_hover_text(t!("settings.hotkeys.clear").to_string())
                            .clicked()
                    {
                        draft.hotkeys.remove(&action);
                        #[cfg(target_os = "macos")]
                        {
                            *lock(&shared.hotkey_verify) = None;
                            testing = None;
                            if let Some(m) = lock(&shared.hotkeys).as_mut() {
                                m.release_verify();
                            }
                        }
                    }
                    let listening = recording == Some(action);
                    let text = if listening {
                        t!("settings.hotkeys.listening").to_string()
                    } else if testing_action {
                        t!("settings.hotkeys.verify").to_string()
                    } else {
                        draft
                            .hotkeys
                            .get(&action)
                            .map(|s| combo_label(s))
                            .unwrap_or_else(|| t!("settings.hotkeys.unset").to_string())
                    };
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(150.0, 22.0), egui::Sense::click());
                    ui.painter().rect(
                        rect,
                        4.0,
                        if listening || testing_action {
                            tk.dock
                        } else {
                            tk.field
                        },
                        egui::Stroke::new(
                            if listening || testing_action {
                                1.5
                            } else {
                                1.0
                            },
                            if listening || testing_action {
                                tk.accent
                            } else {
                                tk.field_border
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional(11.5),
                        if bound || listening || testing_action {
                            tk.text
                        } else {
                            tk.text_dim
                        },
                    );
                    if resp.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        recording = Some(action);
                        // While a field listens, every armed combo is
                        // unregistered — its key press would otherwise
                        // be consumed by the OS hotkey and could never
                        // be re-recorded (and would fire its action).
                        if let Some(m) = lock(&shared.hotkeys).as_mut() {
                            m.suspend();
                        }
                        #[cfg(target_os = "macos")]
                        {
                            // A new recording supersedes any pending
                            // test.
                            *lock(&shared.hotkey_verify) = None;
                            testing = None;
                            if let Some(m) = lock(&shared.hotkeys).as_mut() {
                                m.release_verify();
                            }
                        }
                    }
                });
            });
            let err = errors
                .get(&action)
                .cloned()
                .or_else(|| match sync_errors.get(&action) {
                    Some(crate::hotkeys::HotkeyError::Taken) => {
                        Some(t!("settings.hotkeys.err_taken").to_string())
                    }
                    Some(crate::hotkeys::HotkeyError::Reserved) => {
                        Some(t!("settings.hotkeys.err_reserved").to_string())
                    }
                    None => None,
                });
            if let Some(e) = err {
                ui.label(egui::RichText::new(e).color(tk.accent));
            }
        }
        ui.add_space(4.0);
        if widgets::secondary_button(ui, &t!("settings.hotkeys.reset")).clicked() {
            draft.hotkeys.clear();
            errors.clear();
            #[cfg(target_os = "macos")]
            {
                *lock(&shared.hotkey_verify) = None;
                testing = None;
                if let Some(m) = lock(&shared.hotkeys).as_mut() {
                    m.release_verify();
                }
            }
        }
    });

    ctx.data_mut(|d| d.insert_temp(egui::Id::new(REC_KEY), recording));
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(REC_ERR), errors));
    #[cfg(target_os = "macos")]
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(TEST_KEY), testing));
}
