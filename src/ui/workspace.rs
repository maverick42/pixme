//! Workspace UI: the create/edit dialog shown from the toolbar chip.
//!
//! The naming-mask editor that used to live in the settings window's
//! "File name" page moved here — a mask belongs to a workspace now.

use egui::{Align, Layout, Sense, Stroke, TextEdit, vec2};
use rust_i18n::t;

use crate::app::Shared;
use crate::config::workspace::Workspace;
use crate::ui::{
    icons::{self, Icon},
    theme, widgets,
};

/// The create/edit dialog's draft. `id == None` creates a workspace;
/// `Some(id)` edits that one (`main` included — it renames fine).
pub struct WorkspaceDialog {
    /// `None` → create on confirm, `Some` → edit that workspace.
    pub id: Option<String>,
    /// Field contents (trimmed and validated on confirm).
    pub name: String,
    /// The workspace's naming mask.
    pub mask: String,
    /// Output folder as free text — the Folder icon picks it.
    pub folder: String,
    /// Working copy of the shared preset list (`prefs.filename_templates`):
    /// written back only on confirm, so cancelling leaves it alone.
    pub presets: Vec<String>,
    /// The name field grabs the focus once on open.
    pub focused: bool,
    /// Inline validation message, shown under the fields.
    pub error: Option<String>,
}

impl WorkspaceDialog {
    /// A fresh "New workspace" draft, pre-filled with a unique
    /// localized name, the active workspace's mask and the default
    /// output folder.
    pub fn new_create(s: &crate::config::Settings) -> Self {
        let base = t!("workspace.default_name").to_string();
        Self {
            id: None,
            name: s.unique_workspace_name(&base),
            mask: s.active_workspace().name_mask.clone(),
            folder: s
                .active_workspace()
                .output_dir
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            presets: s.filename_templates.clone(),
            focused: false,
            error: None,
        }
    }

    /// An "Edit" draft cloned from workspace `w`.
    pub fn new_edit(w: &Workspace, s: &crate::config::Settings) -> Self {
        Self {
            id: Some(w.id.clone()),
            name: w.name.clone(),
            mask: w.name_mask.clone(),
            folder: w
                .output_dir
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            presets: s.filename_templates.clone(),
            focused: false,
            error: None,
        }
    }
}

/// What a frame of [`dialog`] decided.
pub enum Outcome {
    /// Still open — the caller keeps `state`.
    Open,
    /// Validated fields; the caller applies them (create or edit).
    Confirm {
        name: String,
        mask: String,
        folder: String,
        presets: Vec<String>,
    },
    /// Dismissed — nothing applies.
    Cancel,
}

/// One frame of the dialog's body — the caller wraps it in
/// `widgets::modal_frame` like every in-viewer dialog.
pub fn dialog(ui: &mut egui::Ui, state: &mut WorkspaceDialog, shared: &Shared) -> Outcome {
    let t = theme::tokens(ui.ctx());
    let name_id = egui::Id::new("pixme-ws-name");

    // The name row — focused once so typing starts immediately.
    ui.horizontal(|ui| {
        ui.label(t!("workspace.field_name").to_string());
        let edit = ui.add(
            TextEdit::singleline(&mut state.name)
                .id(name_id)
                .desired_width(220.0)
                .char_limit(30)
                .font(theme::medium(12.0)),
        );
        if !state.focused {
            edit.request_focus();
            state.focused = true;
        }
    });
    ui.add_space(6.0);

    // The naming-mask row + presets + token legend + live example —
    // the example counts with THIS workspace's counter.
    ui.label(t!("workspace.field_mask").to_string());
    let counter = {
        let s = shared.lock_settings();
        state
            .id
            .as_deref()
            .and_then(|id| s.workspaces.iter().find(|w| w.id == id))
            .map(|w| w.counter)
            .unwrap_or(1)
    };
    mask_editor(ui, &mut state.mask, &mut state.presets, counter);
    ui.add_space(6.0);

    // The output folder — free text + the same picker affordance the
    // settings rows use. Empty = the platform default (~/Pictures).
    ui.label(t!("workspace.field_folder").to_string());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.add(
            TextEdit::singleline(&mut state.folder)
                .desired_width(236.0)
                .hint_text(t!("workspace.folder_hint"))
                .font(theme::medium(12.0)),
        );
        let pick = widgets::icon_button(ui, Icon::FolderOpen, false)
            .on_hover_text(t!("workspace.folder_pick").to_string());
        if pick.clicked()
            && let Some(dir) = rfd::FileDialog::new()
                .set_directory(&state.folder)
                .pick_folder()
        {
            state.folder = dir.to_string_lossy().into_owned();
        }
    });

    // Live validation: the name stays non-empty and unique
    // (case-insensitive, against every OTHER workspace), the mask
    // non-empty too — the example row already shows its expansion.
    let name = state.name.trim();
    let taken = {
        let s = shared.lock_settings();
        s.workspace_name_taken(name, state.id.as_deref())
    };
    if let Some(err) = &state.error {
        ui.add_space(4.0);
        ui.colored_label(t.danger, err.clone());
    } else if taken {
        ui.add_space(4.0);
        ui.colored_label(t.danger, t!("workspace.error_name_taken").to_string());
    }
    ui.add_space(10.0);

    let mut out = Outcome::Open;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_enabled_ui(
            !name.is_empty() && !state.mask.trim().is_empty() && !taken,
            |ui| {
                if widgets::primary_button(ui, &t!("workspace.confirm")).clicked() {
                    out = Outcome::Confirm {
                        name: name.to_owned(),
                        mask: state.mask.trim().to_owned(),
                        folder: state.folder.trim().to_owned(),
                        presets: state.presets.clone(),
                    };
                }
            },
        );
        if widgets::secondary_button(ui, &t!("save.cancel")).clicked() {
            out = Outcome::Cancel;
        }
    });
    out
}

/// The mask row + preset dropdown + token legend + live example —
/// the settings window's File-name page moved here verbatim so the
/// workspace dialog is its only home now.
pub fn mask_editor(ui: &mut egui::Ui, mask: &mut String, presets: &mut Vec<String>, counter: u64) {
    let t = theme::tokens(ui.ctx());
    ui.horizontal(|ui| {
        let editor = egui::TextEdit::singleline(mask)
            .desired_width(220.0)
            .font(egui::TextStyle::Monospace);
        // Filename-hostile chars would break the save itself —
        // strip whatever slips through typing or paste.
        if ui.add(editor).changed() {
            mask.retain(|c| !crate::config::filename::is_forbidden_filename_char(c));
        }
        let presets_btn = widgets::dropdown_button(ui, "");
        egui::Popup::menu(&presets_btn).show(|ui| {
            for preset in presets.iter() {
                if ui.button(egui::RichText::new(preset).monospace()).clicked() {
                    *mask = preset.clone();
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
            let current = mask.trim().to_owned();
            if !current.is_empty() && !presets.contains(&current) {
                presets.push(current);
            }
        }
        if widgets::icon_button(ui, Icon::X, false)
            .on_hover_text(t!("settings.filename.remove").to_string())
            .clicked()
        {
            presets.retain(|p| p != mask);
        }
    });
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(t!("settings.filename.notes").to_string())
            .monospace()
            .color(t.text_dim),
    );
    ui.add_space(8.0);
    let example = crate::config::expand_template(mask, counter, &chrono::Local::now());
    if mask.trim().is_empty() {
        ui.colored_label(t.danger, t!("settings.filename.empty").to_string());
    } else {
        // `.png` already lives in the key — appending it here too
        // produced "… .png.png".
        ui.colored_label(
            t.text,
            t!("settings.filename.example", name = example.as_str()).to_string(),
        );
    }
}

/// The toolbar chip's frame — a compact bordered pill drawn in the
/// strip's free space; the caller interacts with the returned
/// `Response` (left-click list, right-click menu).
pub fn chip_frame(ui: &mut egui::Ui, name: &str, text_w: f32, accented: bool) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let inner = egui::Frame::default()
        .inner_margin(egui::Margin::symmetric(8, 2))
        .fill(if accented { t.accent_soft } else { t.card })
        .stroke(Stroke::new(
            1.0,
            if accented {
                t.accent_border
            } else {
                t.card_border
            },
        ))
        .corner_radius(egui::CornerRadius::same(t.radius_md))
        .show(ui, |ui| {
            // A `horizontal` row claims the FULL available width of its
            // cell, and the frame wraps that row — without capping the
            // max width first, the pill would stretch across the whole
            // gap. The cap = icon 11 + 2×5 spacing + chevron 9 around
            // the fixed `text_w` label (the longest workspace name).
            ui.set_max_width(text_w + 30.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.add(icons::image(
                    Icon::Folder,
                    11.0,
                    if accented { t.accent } else { t.icon },
                ));
                let mut text = egui::RichText::new(name)
                    .color(if accented { t.accent_text } else { t.text })
                    .size(10.5);
                if accented {
                    // egui's `strong()` targets the unregistered
                    // "Strong" family — use the project's SemiBold
                    // family instead.
                    text = text.font(crate::ui::theme::semibold(10.5));
                }
                ui.add_sized(
                    vec2(text_w, 14.0),
                    egui::Label::new(text).truncate().selectable(false),
                );
                ui.add(icons::image(Icon::ChevronDown, 9.0, t.text_dim));
            });
            ui.allocate_space(vec2(0.0, 0.0));
        });
    ui.interact(
        inner.response.rect,
        egui::Id::new("pixme-ws-chip"),
        Sense::click(),
    )
    .on_hover_text(name.to_owned())
}
