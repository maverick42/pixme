//! The pin save-as window: destination folder, format radio
//! (JPG / PNG / WEBP), file name, and an optional "reveal in folder" —
//! the functional reference's save dialog drawn with the app's widgets.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Align, Layout, Ui, Vec2};

use crate::app::Shared;
use crate::capture::Capture;
use crate::export::{ExportError, ImageFormat, file::save_capture_to};
use crate::platform;

use super::{icons::Icon, theme, widgets};

/// What [`show`] decided this frame.
pub enum Outcome {
    /// Nothing happened — keep the dialog open.
    Open,
    /// Enregistrer: write the file (and maybe reveal it), then close.
    Confirm,
    /// Annuler / close button / Esc — close without saving.
    Cancel,
}

/// Width of the right-aligned label column (fits the French labels).
const LABEL_W: f32 = 175.0;

/// State of the pin save window — holds the image so it survives the pin
/// being closed while the dialog is open.
pub struct SaveDialog {
    /// The image to write.
    pub capture: Arc<Capture>,
    /// Destination folder (editable text + browse button).
    pub folder: String,
    /// File name without extension (the format appends its own).
    pub filename: String,
    /// Output format.
    pub format: ImageFormat,
    /// Reveal the saved file in the OS file manager.
    pub reveal: bool,
    /// Error shown inline after a failed save.
    pub error: Option<String>,
    /// When the dialog serves a viewer document (Save Active), the saved
    /// tab index — the app marks it clean on success. `None` for pins.
    pub viewer_tab: Option<usize>,
    /// File this document was loaded/saved from: it may be overwritten
    /// without tripping the "file exists" block (re-saving IS the point).
    pub existing_ok: Option<PathBuf>,
}

impl SaveDialog {
    pub fn new(capture: Arc<Capture>, folder: PathBuf) -> Self {
        Self {
            capture,
            folder: folder.to_string_lossy().into_owned(),
            // Reference-style name: capture_YYMMDD_HHMMSS.
            filename: format!("capture_{}", chrono::Local::now().format("%y%m%d_%H%M%S")),
            format: ImageFormat::Png,
            reveal: false,
            error: None,
            viewer_tab: None,
            existing_ok: None,
        }
    }

    /// Where the file would land with the current fields (the format
    /// radio dictates the extension).
    pub fn candidate_path(&self) -> PathBuf {
        let stem = sanitize(&self.filename);
        let stem = if stem.is_empty() { "capture" } else { &stem };
        PathBuf::from(self.folder.trim()).join(format!("{stem}.{}", self.format.extension()))
    }

    /// True when the candidate path collides with a file on disk that is
    /// not the document's own source — overwriting a foreign file stays
    /// blocked, overwriting its own is what saving means.
    pub fn blocked_by_existing(&self) -> bool {
        let candidate = self.candidate_path();
        if !candidate.exists() {
            return false;
        }
        std::fs::canonicalize(&candidate).ok()
            != self
                .existing_ok
                .as_ref()
                .and_then(|p| std::fs::canonicalize(p).ok())
    }

    /// Writes the file at [`candidate_path`], updates the last-used
    /// folder and reveals it when asked. On success the caller closes the
    /// dialog; on failure the error is shown inline.
    pub fn save(&mut self, shared: &Shared) -> Result<PathBuf, ExportError> {
        let dir = PathBuf::from(self.folder.trim());
        if !dir.is_dir() {
            return Err(ExportError::Failed(t!("save.error_dir").into_owned()));
        }
        let path = self.candidate_path();
        save_capture_to(&self.capture, &path, self.format)?;
        if self.reveal {
            platform::reveal_in_file_manager(&path);
        }
        // Persist the folder so the next save (and next session) reopens here.
        let mut s = shared.lock_settings();
        s.last_save_dir = Some(dir);
        s.push_recent(path.clone());
        s.save();
        Ok(path)
    }
}

/// The dialog body for one frame.
pub fn show(ui: &mut Ui, state: &mut SaveDialog) -> Outcome {
    let t = theme::tokens(ui.ctx());
    let mut outcome = Outcome::Open;

    // Explicit fill: the app-wide clear color is transparent (the anchor
    // and the floating bar need it), so windows must paint their own.
    egui::CentralPanel::default()
        .frame(
            egui::Frame::default()
                .inner_margin(egui::Margin::symmetric(20, 14))
                .fill(t.dock),
        )
        .show(ui, |ui| {
            egui::Grid::new("pixme-save-grid")
                .num_columns(2)
                .spacing([20.0, 12.0])
                .show(ui, |ui| {
                    // -- destination folder -------------------------------
                    right_label(ui, t!("save.location"));
                    ui.horizontal(|ui| {
                        let edit = egui::TextEdit::singleline(&mut state.folder)
                            .desired_width(280.0)
                            .font(theme::medium(12.0));
                        ui.add(edit);
                        if widgets::icon_button(ui, Icon::Folder, false)
                            .on_hover_text(t!("save.browse"))
                            .clicked()
                            && let Some(dir) = rfd::FileDialog::new()
                                .set_directory(&state.folder)
                                .pick_folder()
                        {
                            state.folder = dir.to_string_lossy().into_owned();
                        }
                    });
                    ui.end_row();

                    // -- format radio -------------------------------------
                    right_label(ui, t!("save.format"));
                    ui.horizontal(|ui| {
                        for f in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Webp] {
                            ui.radio_value(&mut state.format, f, f.label());
                            ui.add_space(6.0);
                        }
                    });
                    ui.end_row();

                    // -- file name -----------------------------------------
                    right_label(ui, t!("save.filename"));
                    let name = egui::TextEdit::singleline(&mut state.filename)
                        .desired_width(280.0)
                        .font(theme::medium(12.0));
                    ui.add(name);
                    ui.end_row();

                    // -- reveal ---------------------------------------------
                    right_label(ui, t!("save.reveal_label"));
                    ui.checkbox(&mut state.reveal, t!("save.open_folder"));
                    ui.end_row();
                });

            // Live existence check: saving over a FOREIGN file is blocked
            // (the document's own source is exempt — re-save overwrites it).
            let taken = state.blocked_by_existing();
            if let Some(err) = &state.error {
                ui.add_space(6.0);
                ui.colored_label(t.danger, err);
            }
            if taken {
                ui.add_space(6.0);
                ui.colored_label(t.warning, t!("save.exists"));
            }

            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(!taken, |ui| {
                    if widgets::primary_button(ui, &t!("save.confirm")).clicked() {
                        outcome = Outcome::Confirm;
                    }
                });
                if widgets::secondary_button(ui, &t!("save.cancel")).clicked() {
                    outcome = Outcome::Cancel;
                }
            });
        });

    outcome
}

/// Right-aligned label cell for the two-column form.
fn right_label(ui: &mut Ui, text: impl Into<String>) {
    ui.allocate_ui_with_layout(
        Vec2::new(LABEL_W, 20.0),
        Layout::right_to_left(Align::Center),
        |ui| {
            ui.label(text.into());
        },
    );
}

/// Turns free text into a file stem: strips the extension and every
/// path-hostile character.
fn sanitize(name: &str) -> String {
    let stem = Path::new(name.trim())
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    stem.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{MonitorInfo, PhysicalRect};

    fn dialog_in(dir: &Path) -> SaveDialog {
        let capture = Capture {
            image: std::sync::Arc::new(image::RgbaImage::new(1, 1)),
            scale_factor: 1.0,
            monitor: MonitorInfo {
                id: 0,
                name: String::new(),
                rect: PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                scale_factor: 1.0,
                is_primary: true,
            },
        };
        SaveDialog::new(Arc::new(capture), dir.to_path_buf())
    }

    /// The exists-block ignores the document's own source path but keeps
    /// protecting every other file on disk.
    #[test]
    fn own_file_is_exempt_from_the_exists_block() {
        let dir = std::env::temp_dir().join(format!("pixme-dlg-{}", crate::editor::meta::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shot.png");
        std::fs::write(&file, b"png").unwrap();
        let other = dir.join("other.png");
        std::fs::write(&other, b"png").unwrap();

        let mut d = dialog_in(&dir);
        d.filename = "shot".to_owned();
        assert!(d.blocked_by_existing(), "a foreign existing file blocks");
        d.existing_ok = Some(file);
        assert!(
            !d.blocked_by_existing(),
            "the document's own file may be overwritten"
        );
        d.filename = "other".to_owned();
        assert!(
            d.blocked_by_existing(),
            "another existing file still blocks"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
