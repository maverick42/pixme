//! The session viewer — PixMe's main window.
//!
//! One tab per capture of the session, a "+" action that hands back to the
//! floating bar for a new capture, and the translated Save/Discard/Cancel
//! confirmation when a dirty session would be lost. On Windows the viewer
//! also draws the application's menu strip (no global menu bar exists);
//! on macOS the same actions live in the native PixMe/Capture/File menus.
//! The toolbar carries the file/edit actions (open, save, select, zoom,
//! resize, clipboard, print, close); Draw lands in the next phase.

use std::collections::VecDeque;

use egui::{
    Align, CentralPanel, Color32, CornerRadius, Layout, Margin, Pos2, Rect, Response, Sense,
    Stroke, StrokeKind, Vec2, ViewportCommand,
};
use rust_i18n::t;

use crate::app::{CaptureMode, Command, Shared};
use crate::editor::Document;
use crate::editor::history::AnnoEdit;
use crate::ui::icons::{self, Icon};
use crate::ui::{theme, widgets};

/// One open capture: its document plus the (rebuilt-on-revision) texture.
pub struct Tab {
    /// Tab label.
    pub title: String,
    /// The capture + annotations.
    pub doc: Document,
    /// Whether the document matches what's on disk (opened file, or saved
    /// since the last edit). Drives the session's `dirty` flag.
    pub saved: bool,
    /// `(revision, texture)` cache living with the tab — closing a tab
    /// drops its texture and a later tab can never inherit a stale one.
    texture: Option<(u64, egui::TextureHandle)>,
    /// Reversible pixel mutations, oldest → newest (session-only).
    undo: VecDeque<PixelOp>,
    /// Ops undone, reappliable until the next mutation clears them.
    redo: VecDeque<PixelOp>,
    /// Chronological annotation edits — the Draw window moves them in
    /// and out on open/commit so they survive editor re-entry; a
    /// pixel-level flatten clears them for good.
    pub anno_undo: VecDeque<AnnoEdit>,
    /// Redo side of the annotation stacks.
    pub anno_redo: VecDeque<AnnoEdit>,
    /// Password accepted while this tab is visible — leaving the tab
    /// re-locks it (`false` again), so a protected capture asks for its
    /// password on every activation.
    pub unlocked: bool,
}

/// How many pixel mutations stay undoable per tab. Full-image ops keep
/// one pre-image each (~20 MB on a Retina capture), so the cap bounds
/// the worst case.
const HISTORY_CAP: usize = 10;

/// One reversible pixel-level mutation of a tab's base image.
enum PixelOp {
    /// Resampled to `size`; `before` is the whole pre-resize image.
    Resize {
        before: image::RgbaImage,
        size: (u32, u32),
        antialias: bool,
    },
}

impl PixelOp {
    /// Reverts the mutation on `doc`'s base image.
    fn undo(&self, doc: &mut Document) {
        match self {
            Self::Resize { before, .. } => {
                doc.base.image = before.clone();
            }
        }
    }

    /// Re-applies the mutation on `doc`'s base image after an undo.
    fn redo(&self, doc: &mut Document) {
        match self {
            Self::Resize {
                size, antialias, ..
            } => {
                doc.base.image = image::imageops::resize(
                    &doc.base.image,
                    size.0,
                    size.1,
                    if *antialias {
                        image::imageops::FilterType::Lanczos3
                    } else {
                        image::imageops::FilterType::Nearest
                    },
                );
            }
        }
    }
}

/// The Redimensionner dialog's draft — explicit pixels only (the percent
/// mode comes back later through the button's menu).
struct ResizeDialog {
    /// Draft width, px.
    width: String,
    /// Draft height, px.
    height: String,
    /// Keep the aspect ratio while editing width/height.
    keep_ratio: bool,
    /// Anti-aliased resample (Lanczos3) vs nearest-neighbour.
    anti_alias: bool,
    /// The draft failed validation — show the inline error.
    invalid: bool,
}

/// How the canvas is scaled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum ZoomMode {
    /// Fit the whole image in the window (never upscales).
    #[default]
    Fit,
    /// Fill the window width (scrolls vertically when taller).
    FitWidth,
    /// Explicit factor (1.0 = actual size).
    Factor(f32),
}

/// Session state rendered inside the viewer viewport.
#[derive(Default)]
pub struct Viewer {
    /// One tab per captured document.
    pub tabs: Vec<Tab>,
    /// Index of the visible tab.
    pub active: usize,
    /// Whether the session holds unsaved captures or annotations.
    pub dirty: bool,
    /// Show the Save/Discard/Cancel confirmation.
    pub confirm_close: bool,
    /// After the confirmation resolves, quit instead of going Idle.
    pub confirm_then_quit: bool,
    /// Running tab counter so titles stay unique after closes.
    next_title: u32,
    /// Canvas zoom: fit, fit-width, or an explicit factor.
    zoom: ZoomMode,
    /// Scale applied last frame — drives the zoom label and ± steps.
    eff_scale: f32,
    /// The resize dialog, open while Some.
    resize: Option<ResizeDialog>,
    /// The rename prompt's draft, open while Some.
    rename: Option<RenameState>,
    /// The protection dialog (set/change/remove password), open while Some.
    protect: Option<ProtectDialog>,
    /// The "delete the file on disk" confirmation, open while Some.
    confirm_delete: Option<DeleteFileDialog>,
    /// Draft of the locked tab's password prompt.
    unlock_draft: String,
    /// The last unlock attempt failed — show the inline error.
    unlock_error: bool,
    /// Last measured horizontal offset of the tab strip — the ◀ ▶
    /// buttons scroll relative to it.
    tab_scroll_pos: f32,
    /// One-shot horizontal offset applied to the tab strip next frame.
    tab_scroll_to: Option<f32>,
    /// A tab activation asks the strip to scroll it into view once.
    scroll_active_into_view: bool,
    /// The chips were wider than the strip last frame — shows ◀ ▶.
    tabs_overflow: bool,
    /// Same scroll bookkeeping for the toolbar band.
    tool_scroll_pos: f32,
    /// One-shot horizontal offset applied to the toolbar next frame.
    tool_scroll_to: Option<f32>,
    /// The toolbar was wider than its band last frame — shows ◀ ▶.
    tools_overflow: bool,
}

/// The rename prompt: draft + whether it already failed uniqueness.
struct RenameState {
    /// Field content.
    draft: String,
    /// Last Apply hit a duplicate/empty name — inline error shown.
    taken: bool,
}

/// Which field of the protection dialog failed validation.
enum ProtectError {
    /// The current password did not match.
    Current,
    /// New and confirm differ.
    Mismatch,
}

/// The Protection… dialog: set a password on an unprotected tab, or —
/// after proving the current one — change it or clear it (blank new
/// password = remove).
struct ProtectDialog {
    /// The tab it edits.
    tab: usize,
    /// "Current password" field (protected tabs only).
    current: String,
    /// "New password" field.
    new: String,
    /// Confirmation field.
    confirm: String,
    /// Last Apply failed — which error to show.
    error: Option<ProtectError>,
}

/// The "Supprimer le fichier" confirmation — the file on disk is the
/// only thing at stake; the tab stays open either way.
struct DeleteFileDialog {
    /// The tab whose `source_path` would be deleted.
    tab: usize,
    /// The last delete attempt failed — inline error shown.
    error: Option<String>,
}

impl Tab {
    /// Bakes annotations into the base image and discards object-level
    /// history — the documented "aplatie" semantics of viewer pixel ops.
    fn flatten(&mut self) {
        if self.doc.annotations.is_empty() && self.anno_undo.is_empty() && self.anno_redo.is_empty()
        {
            return;
        }
        self.doc.base.image = crate::editor::render::render_to_image(&self.doc);
        self.doc.annotations.clear();
        self.anno_undo.clear();
        self.anno_redo.clear();
    }

    /// Stacks a reversible mutation — redo dies, the oldest op beyond
    /// the cap drops off.
    fn push_op(&mut self, op: PixelOp) {
        self.undo.push_back(op);
        if self.undo.len() > HISTORY_CAP {
            self.undo.pop_front();
        }
        self.redo.clear();
    }
}

impl Viewer {
    /// Adds `doc` as a fresh tab, selects it and marks the session dirty.
    /// Returns the generated title.
    pub fn push_document(&mut self, doc: Document) -> String {
        self.next_title += 1;
        let title = t!("capture.default_name", n = self.next_title).to_string();
        self.push_named_document(doc, title.clone(), false);
        title
    }

    /// Adds `doc` as a tab with an explicit title and saved flag — used by
    /// Open (file name, already on disk) and Paste (untitled, unsaved).
    /// Two captures can never share a name: a clash gets a ` (n)` suffix.
    pub fn push_named_document(&mut self, doc: Document, title: String, saved: bool) {
        let title = self.unique_title(&title);
        self.tabs.push(Tab {
            title,
            doc,
            saved,
            texture: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            anno_undo: VecDeque::new(),
            anno_redo: VecDeque::new(),
            unlocked: false,
        });
        // A new document gets a fresh view — a stale zoom would
        // leak onto unrelated pixels — and the tab we leave re-locks.
        let new = self.tabs.len() - 1;
        self.set_active(new);
        self.recompute_dirty();
    }

    /// Test helper: a tab whose document wraps a 1×1 capture (goes
    /// through the unique-title rule like the real push paths).
    #[cfg(test)]
    pub fn push_stub(&mut self, title: &str) {
        use crate::capture::{Capture, MonitorInfo, PhysicalRect};
        let title = self.unique_title(title);
        self.tabs.push(Tab {
            title,
            doc: Document::new(Capture {
                image: image::RgbaImage::new(1, 1),
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
            }),
            saved: false,
            texture: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            anno_undo: VecDeque::new(),
            anno_redo: VecDeque::new(),
            unlocked: false,
        });
    }

    /// The session's dirty flag = any tab not written to disk.
    pub fn recompute_dirty(&mut self) {
        self.dirty = self.tabs.iter().any(|t| !t.saved);
    }

    /// Marks `index` saved at `path` and refreshes the session flag —
    /// called after a save-as dialog completes for a viewer document.
    /// The meta records the location and the timestamp ("was saved at
    /// least once, and when") for the future database restore.
    pub fn mark_saved(&mut self, index: usize, path: std::path::PathBuf) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.saved = true;
            tab.doc.meta.source_path = Some(path);
            tab.doc.meta.saved_at = Some(chrono::Local::now());
            // Saving a drawn-on document bakes pixels the op history
            // knows nothing about — the chronology ends there.
            if !tab.doc.annotations.is_empty() {
                tab.undo.clear();
                tab.redo.clear();
            }
            self.recompute_dirty();
        }
    }

    /// Index of the active tab, clamped into range (`None` when empty).
    fn active_index(&self) -> Option<usize> {
        if self.tabs.is_empty() {
            None
        } else {
            Some(self.active.min(self.tabs.len() - 1))
        }
    }

    /// The active tab's document, for dispatch (`SaveActive`).
    pub fn active_doc(&self) -> Option<&Document> {
        self.active_index().map(|i| &self.tabs[i].doc)
    }

    /// `(tab index, doc clone, anno stacks)` — opening Draw snapshots
    /// the document while its history stays on the tab until commit.
    /// A floating paste flattens first so it isn't lost by the session.
    #[allow(clippy::type_complexity)]
    pub fn draw_parts(
        &mut self,
    ) -> Option<(
        usize,
        String,
        Document,
        VecDeque<AnnoEdit>,
        VecDeque<AnnoEdit>,
    )> {
        let i = self.active_index()?;
        let tab = &self.tabs[i];
        Some((
            i,
            tab.title.clone(),
            tab.doc.clone(),
            tab.anno_undo.clone(),
            tab.anno_redo.clone(),
        ))
    }

    /// Draw's OK: the session's document and annotation stacks replace
    /// the tab's — objects stay re-editable on the next entry.
    pub fn commit_draw(
        &mut self,
        tab: usize,
        doc: Document,
        undo: VecDeque<AnnoEdit>,
        redo: VecDeque<AnnoEdit>,
    ) {
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        t.doc = doc;
        t.doc.revision += 1;
        t.anno_undo = undo;
        t.anno_redo = redo;
        t.saved = false;
        self.dirty = true;
    }

    /// `(index, title, base capture)` of the active tab — everything the
    /// save-as dialog needs to own (the capture is cloned).
    pub fn save_target(&self) -> Option<(usize, String, crate::capture::Capture)> {
        self.save_target_at(self.active_index()?)
    }

    /// Same as [`Self::save_target`] for an explicit tab — the
    /// close-request confirmation saves a tab that is not the active one.
    pub fn save_target_at(&self, index: usize) -> Option<(usize, String, crate::capture::Capture)> {
        let tab = self.tabs.get(index)?;
        let mut capture = tab.doc.base.clone();
        capture.image = crate::editor::render::render_to_image(&tab.doc);
        Some((index, tab.title.clone(), capture))
    }

    /// Opens the rename prompt on the active tab (File → Rename).
    pub fn open_rename(&mut self) {
        if let Some(title) = self.active_title() {
            self.rename = Some(RenameState {
                draft: title,
                taken: false,
            });
        }
    }

    /// What Copier puts on the clipboard: the whole document image.
    pub fn copy_image(&mut self) -> Option<image::RgbaImage> {
        let i = self.active_index()?;
        Some(crate::editor::render::render_to_image(&self.tabs[i].doc))
    }

    /// The whole document image, for print.
    pub fn active_image(&self) -> Option<image::RgbaImage> {
        let i = self.active_index()?;
        Some(crate::editor::render::render_to_image(&self.tabs[i].doc))
    }

    /// The active tab's title, for the rename prompt's initial value.
    pub fn active_title(&self) -> Option<String> {
        self.active_index().map(|i| self.tabs[i].title.clone())
    }

    /// Renames the active tab (the File → Rename command).
    pub fn set_active_title(&mut self, title: String) {
        if let Some(i) = self.active_index() {
            self.tabs[i].title = title;
        }
    }

    /// ⌘Z — reverts the newest pixel mutation of the active tab.
    pub fn undo_active(&mut self) -> bool {
        let Some(i) = self.active_index() else {
            return false;
        };
        let Some(op) = self.tabs[i].undo.pop_back() else {
            return false;
        };
        let tab = &mut self.tabs[i];
        op.undo(&mut tab.doc);
        tab.doc.revision += 1;
        tab.saved = false;
        tab.redo.push_back(op);
        self.dirty = true;
        true
    }

    /// ⇧⌘Z / Ctrl+Y — re-applies the newest undone mutation.
    pub fn redo_active(&mut self) -> bool {
        let Some(i) = self.active_index() else {
            return false;
        };
        let Some(op) = self.tabs[i].redo.pop_back() else {
            return false;
        };
        let tab = &mut self.tabs[i];
        op.redo(&mut tab.doc);
        tab.doc.revision += 1;
        tab.saved = false;
        tab.undo.push_back(op);
        self.dirty = true;
        true
    }

    /// Resamples the active document to `w × h` physical px (the resize
    /// dialog's Apply); `antialias` picks Lanczos3 over nearest-neighbour.
    /// Annotations are dropped: their physical-px coordinates would need
    /// a translation pass the editor model doesn't have yet.
    fn resize_active(&mut self, w: u32, h: u32, antialias: bool) {
        let Some(i) = self.active_index() else { return };
        if w == 0 || h == 0 {
            return;
        }
        let tab = &mut self.tabs[i];
        tab.flatten();
        let before = tab.doc.base.image.clone();
        tab.doc.base.image = image::imageops::resize(
            &tab.doc.base.image,
            w,
            h,
            if antialias {
                image::imageops::FilterType::Lanczos3
            } else {
                image::imageops::FilterType::Nearest
            },
        );
        tab.push_op(PixelOp::Resize {
            before,
            size: (w, h),
            antialias,
        });
        tab.doc.revision += 1;
        tab.saved = false;
        self.dirty = true;
    }

    /// Adds a blank document ("Nouveau") — auto-titled by the shared
    /// counter so several news stay distinguishable.
    pub fn push_blank_document(&mut self, doc: Document) -> String {
        self.next_title += 1;
        let title = format!("{} {}", t!("doc.new_title"), self.next_title);
        self.push_named_document(doc, title.clone(), false);
        title
    }

    /// Resets the per-view transient state (tab switch / doc mutation).
    fn reset_view(&mut self) {
        self.zoom = ZoomMode::Fit;
    }

    /// Makes `index` the visible tab. Leaving a tab re-locks it: a
    /// protected capture asks for its password on EVERY activation, not
    /// once per session (the spec's rule).
    fn set_active(&mut self, index: usize) {
        self.active = index;
        for tab in &mut self.tabs {
            tab.unlocked = false;
        }
        self.unlock_draft.clear();
        self.unlock_error = false;
        self.scroll_active_into_view = true;
        self.reset_view();
    }

    /// Whether `index` is a protected tab still waiting for its password.
    fn is_locked(&self, index: usize) -> bool {
        self.tabs
            .get(index)
            .is_some_and(|t| t.doc.meta.is_protected() && !t.unlocked)
    }

    /// `true` when another tab already carries `name` (trimmed,
    /// case-insensitive) — two captures can never share a name.
    fn title_taken(&self, name: &str, except: Option<usize>) -> bool {
        let name = name.trim().to_lowercase();
        self.tabs
            .iter()
            .enumerate()
            .any(|(i, t)| Some(i) != except && t.title.trim().to_lowercase() == name)
    }

    /// `name` if free, else `name (2)`, `name (3)`, … — the unique-name
    /// rule enforced at every tab creation path.
    fn unique_title(&self, base: &str) -> String {
        let base = base.trim();
        if !self.title_taken(base, None) {
            return base.to_owned();
        }
        for n in 2.. {
            let candidate = format!("{base} ({n})");
            if !self.title_taken(&candidate, None) {
                return candidate;
            }
        }
        unreachable!()
    }

    /// Drops `indices` (sorted or not), keeps `active` valid and relocks
    /// the newly surfaced tab when the active one was removed. A capture
    /// ALWAYS closes without a warning — the red marker already told the
    /// user the tab was unsaved (the reference's rule).
    pub fn close_tabs(&mut self, indices: &[usize]) {
        let mut sorted: Vec<usize> = indices.to_vec();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        sorted.dedup();
        let below_active = sorted.iter().filter(|&&i| i < self.active).count();
        let active_removed = sorted.contains(&self.active);
        for i in sorted {
            if i < self.tabs.len() {
                self.tabs.remove(i);
            }
        }
        if active_removed {
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
            for tab in &mut self.tabs {
                tab.unlocked = false;
            }
            self.unlock_draft.clear();
            self.unlock_error = false;
            self.reset_view();
        } else {
            // `active` counted the removed tabs below it.
            self.active = self
                .active
                .saturating_sub(below_active)
                .min(self.tabs.len().saturating_sub(1));
        }
        self.recompute_dirty();
    }

    /// Duplicates `index` into a new tab: new id and capture date, same
    /// password (a copy of protected content stays protected), a ` (n)`
    /// unique title — the document clone carries annotations and history.
    fn duplicate_tab(&mut self, index: usize) {
        let Some(src) = self.tabs.get(index) else {
            return;
        };
        let mut doc = src.doc.clone();
        doc.meta.id = crate::editor::meta::new_id();
        doc.meta.captured_at = chrono::Local::now();
        // The copy has never been written itself.
        doc.meta.saved_at = None;
        doc.meta.source_path = None;
        let title = self.unique_title(&src.title);
        self.tabs.push(Tab {
            title,
            doc,
            saved: false,
            texture: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            anno_undo: src.anno_undo.clone(),
            anno_redo: src.anno_redo.clone(),
            unlocked: false,
        });
        let new = self.tabs.len() - 1;
        self.set_active(new);
        self.dirty = true;
    }

    /// Deletes `tab`'s file on disk — the tab itself stays open, now
    /// unsaved again (the file it pointed at is gone).
    fn delete_tab_file(&mut self, tab: usize) -> Result<(), std::io::Error> {
        let Some(path) = self
            .tabs
            .get(tab)
            .and_then(|t| t.doc.meta.source_path.clone())
        else {
            return Ok(());
        };
        std::fs::remove_file(&path)?;
        let t = &mut self.tabs[tab];
        t.doc.meta.source_path = None;
        t.doc.meta.saved_at = None;
        t.saved = false;
        self.recompute_dirty();
        Ok(())
    }

    /// Draws the viewer for this frame. `shared` routes actions back to the
    /// state machine (the deferred-viewport callback cannot borrow the app).
    pub fn show(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) {
            // The state machine owns this window's lifetime: always cancel
            // and let dispatch() decide whether closing is allowed.
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            shared.push(Command::ViewerClosed);
        }
        let modal_open = self.confirm_close
            || self.resize.is_some()
            || self.rename.is_some()
            || self.protect.is_some()
            || self.confirm_delete.is_some()
            || shared.modal_child_open();
        // Canvas keyboard: ⌘/Ctrl+C copies the document. The copy
        // convention works everywhere in the window — text edits keep
        // their own handling.
        if !modal_open && !ctx.egui_wants_keyboard_input() {
            // `egui_winit` translates ⌘/Ctrl+C into `Event::Copy` instead of
            // `Event::Key(Key::C)`, so check both.
            let copy = ctx.input(|i| {
                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                    || (i.modifiers.command && i.key_pressed(egui::Key::C))
            });
            if copy {
                shared.push(Command::CopyActive);
                // The push lands mid-frame: wake the next pass so
                // pump() dispatches it even without mouse activity.
                ctx.request_repaint();
            }
            // ⇧⌘Z before ⌘Z — the consumed press must not fall
            // through to the plain-undo arm.
            if ctx.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::Z,
                )
            }) {
                shared.push(Command::RedoActive);
                ctx.request_repaint();
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
                shared.push(Command::UndoActive);
                ctx.request_repaint();
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)) {
                // Ctrl+Y — the Windows redo convention.
                shared.push(Command::RedoActive);
                ctx.request_repaint();
            }
        }
        // Explicit fill: the app-wide clear color is transparent (the anchor
        // and the floating bar need it), so windows must paint their own.
        let t = theme::tokens(&ctx);
        egui::Panel::bottom("viewer.status")
            .frame(
                egui::Frame::default()
                    .inner_margin(Margin::symmetric(12, 4))
                    .fill(t.dock),
            )
            .show(ui, |ui| {
                self.status_strip(ui);
            });
        CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .inner_margin(Margin::ZERO)
                    .fill(t.dock),
            )
            .show(ui, |ui| {
                if !shared.has_native_app_menu() {
                    self.menu_strip(ui, shared);
                    widgets::hairline(ui);
                }
                self.toolbar_strip(ui, shared);
                widgets::hairline(ui);
                self.tab_strip(ui, shared);
                widgets::hairline(ui);
                self.content(ui);
                // Modal behaviour for child windows living in other
                // viewports (save-as, settings): dim the whole panel and
                // eat every click while one is up.
                if shared.modal_child_open() {
                    let rect = ui.max_rect();
                    ui.painter()
                        .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(70));
                    ui.allocate_rect(rect, Sense::click_and_drag());
                }
            });
        self.confirm_dialog(&ctx, shared);
        self.resize_dialog(&ctx);
        self.rename_dialog(&ctx);
        self.protect_dialog(&ctx);
        self.delete_file_dialog(&ctx);
    }

    /// The bottom status strip: capture count, active image dimensions
    /// and the app version.
    fn status_strip(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.horizontal(|ui| {
            let n = self.tabs.len();
            let i = self.active_index().map(|x| x + 1).unwrap_or(0);
            ui.label(
                egui::RichText::new(t!("vbar.status.count", i = i, n = n).to_string())
                    .color(t.text_dim)
                    .size(10.0),
            );
            if let Some((w, h)) = self.active_dims() {
                ui.label(
                    egui::RichText::new(format!("{w}×{h}"))
                        .color(t.text_dim)
                        .size(10.0),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .color(t.text_faint)
                        .size(10.0),
                );
            });
        });
    }

    /// The active tab's image dimensions, in physical pixels.
    fn active_dims(&self) -> Option<(u32, u32)> {
        let index = self.active_index()?;
        let tab = &self.tabs[index];
        Some((tab.doc.base.image.width(), tab.doc.base.image.height()))
    }

    /// The action band: large icon-over-label buttons — the functional
    /// reference's toolbar in our widget style. Ouvrir, Rectangle and the
    /// zoom button carry a caret zone that opens their popup menu (the
    /// main click still does the primary action). Zoom/select/resize
    /// mutate viewer state directly; shared actions go through
    /// [`Command`]. No keyboard shortcuts are exposed here.
    fn toolbar_strip(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let has_doc = self.active_index().is_some();
        // Read once up-front — the Ouvrir menu's last item greys without it.
        let has_last_dir = {
            let s = shared.lock_settings();
            s.last_open_dir.is_some() || s.last_save_dir.is_some()
        };
        egui::Frame::default()
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                // Identical to `tab_strip`: wheel scrolls without a
                // scrollbar, ◀ ▶ appear at the right edge on overflow.
                ui.style_mut().always_scroll_the_only_direction = true;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
                    let right_w = if self.tools_overflow {
                        2.0 * (widgets::BUTTON_SIZE + 4.0)
                    } else {
                        0.0
                    };
                    let strip_w = (ui.available_width() - right_w).max(60.0);
                    let mut area = egui::ScrollArea::horizontal()
                        .id_salt("pixme-toolbar")
                        .auto_shrink([true, true])
                        .max_width(strip_w)
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                        );
                    if let Some(x) = self.tool_scroll_to.take() {
                        area = area.scroll_offset(egui::vec2(x, 0.0));
                    }
                    let out = area.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
                            // Ouvrir ▾ — file picker on the main click.
                            let r = widgets::tool_button(
                                ui,
                                Icon::FolderOpen,
                                &t!("vbar.open.label"),
                                true,
                                false,
                                true,
                            );
                            let menu_clicked = r.clicked() && widgets::caret_hit(&r, true);
                            if r.clicked() && !menu_clicked {
                                shared.push(Command::OpenFile);
                            }
                            // Menus render every frame, forced open on the
                            // caret-click frame — see `tool_popup`.
                            open_menu(ui, &r, shared, has_last_dir, menu_clicked);
                            if widgets::tool_button(
                                ui,
                                Icon::Save,
                                &t!("vbar.save.label"),
                                false,
                                false,
                                has_doc,
                            )
                            .clicked()
                            {
                                shared.push(Command::SaveActive);
                            }
                            widgets::vline(ui, widgets::TOOL_H - 10.0);
                            if widgets::tool_button(
                                ui,
                                Icon::ZoomIn,
                                &t!("vbar.zoom_in.label"),
                                false,
                                false,
                                has_doc,
                            )
                            .clicked()
                            {
                                self.zoom =
                                    ZoomMode::Factor((self.scale_for_step() * 1.25).min(8.0));
                            }
                            if widgets::tool_button(
                                ui,
                                Icon::ZoomOut,
                                &t!("vbar.zoom_out.label"),
                                false,
                                false,
                                has_doc,
                            )
                            .clicked()
                            {
                                self.zoom =
                                    ZoomMode::Factor((self.scale_for_step() / 1.25).max(0.05));
                            }
                            // The zoom button shows the live percent; its caret
                            // opens the mode/preset menu, main click = 100%.
                            let zoom_label = self.zoom_label();
                            let r = widgets::tool_button(
                                ui,
                                Icon::Scan,
                                &zoom_label,
                                true,
                                false,
                                has_doc,
                            );
                            let menu_clicked = r.clicked() && widgets::caret_hit(&r, true);
                            if r.clicked() && !menu_clicked {
                                self.zoom = ZoomMode::Factor(1.0);
                            }
                            zoom_menu(ui, &r, self, menu_clicked);
                            widgets::vline(ui, widgets::TOOL_H - 10.0);
                            if widgets::tool_button(
                                ui,
                                Icon::Pencil,
                                &t!("vbar.draw.label"),
                                false,
                                false,
                                has_doc,
                            )
                            .clicked()
                            {
                                shared.push(crate::app::Command::OpenDraw);
                            }
                            if widgets::tool_button(
                                ui,
                                Icon::Scaling,
                                &t!("vbar.resize.label"),
                                false,
                                false,
                                has_doc,
                            )
                            .clicked()
                                && let Some(i) = self.active_index()
                            {
                                let (w, h) = (
                                    self.tabs[i].doc.base.image.width(),
                                    self.tabs[i].doc.base.image.height(),
                                );
                                self.resize = Some(ResizeDialog {
                                    width: w.to_string(),
                                    height: h.to_string(),
                                    keep_ratio: true,
                                    anti_alias: true,
                                    invalid: false,
                                });
                            }
                            widgets::vline(ui, widgets::TOOL_H - 10.0);
                            if widgets::tool_button(
                                ui,
                                Icon::Copy,
                                &t!("vbar.copy.label"),
                                false,
                                false,
                                has_doc,
                            )
                            .clicked()
                            {
                                shared.push(Command::CopyActive);
                            }
                            widgets::vline(ui, widgets::TOOL_H - 10.0);
                            if widgets::tool_button(
                                ui,
                                Icon::Printer,
                                &t!("vbar.print.label"),
                                false,
                                false,
                                has_doc,
                            )
                            .clicked()
                            {
                                shared.push(Command::PrintActive);
                            }
                            if widgets::tool_button(
                                ui,
                                Icon::X,
                                &t!("vbar.close.label"),
                                false,
                                false,
                                true,
                            )
                            .clicked()
                            {
                                shared.push(Command::ViewerClosed);
                            }
                        });
                    });
                    self.tool_scroll_pos = out.state.offset.x;
                    self.tools_overflow = out.content_size.x > out.inner_rect.width() + 0.5;
                    if self.tools_overflow {
                        let max_off = (out.content_size.x - out.inner_rect.width()).max(0.0);
                        ui.add_enabled_ui(self.tool_scroll_pos > 0.5, |ui| {
                            if widgets::icon_button(ui, Icon::ChevronLeft, false).clicked() {
                                self.tool_scroll_to = Some((self.tool_scroll_pos - 200.0).max(0.0));
                            }
                        });
                        ui.add_enabled_ui(self.tool_scroll_pos < max_off - 0.5, |ui| {
                            if widgets::icon_button(ui, Icon::ChevronRight, false).clicked() {
                                self.tool_scroll_to = Some(self.tool_scroll_pos + 200.0);
                            }
                        });
                    }
                });
            });
    }

    /// The zoom button's caption — the live factor as a percent (first
    /// frame, before `content()` measures, defaults to 100%).
    fn zoom_label(&self) -> String {
        if self.eff_scale <= 0.0 {
            return "100%".to_owned();
        }
        format!("{}%", (self.eff_scale * 100.0).round() as u32)
    }

    /// The current effective scale for the ± steps (1.0 before the first
    /// measured frame).
    fn scale_for_step(&self) -> f32 {
        if self.eff_scale > 0.0 {
            self.eff_scale
        } else {
            1.0
        }
    }

    /// egui menu strip for OSes without a native menu bar (Windows). Same
    /// commands as the native menus — the only difference is where they
    /// render.
    fn menu_strip(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
            menu_button(ui, t!("menu.file").as_ref(), |ui| {
                menu_item(ui, t!("menu.file.open").as_ref(), || {
                    shared.push(Command::OpenFile)
                });
                menu_item(ui, t!("menu.app.import_clipboard").as_ref(), || {
                    shared.push(Command::ImportClipboard)
                });
                ui.separator();
                menu_item(ui, t!("menu.file.save").as_ref(), || {
                    shared.push(Command::SaveActive)
                });
                menu_item(ui, t!("menu.file.save_all").as_ref(), || {
                    shared.push(Command::SaveAll)
                });
                menu_item(ui, t!("menu.file.rename").as_ref(), || {
                    shared.push(Command::RenameActive)
                });
                ui.separator();
                menu_item(ui, t!("menu.file.print").as_ref(), || {
                    shared.push(Command::PrintActive)
                });
                ui.separator();
                menu_item(ui, t!("menu.app.quit").as_ref(), || {
                    shared.push(Command::Quit)
                });
            });
            menu_button(ui, t!("menu.edit").as_ref(), |ui| {
                menu_item(ui, t!("menu.edit.undo").as_ref(), || {
                    shared.push(Command::UndoActive)
                });
                menu_item(ui, t!("menu.edit.redo").as_ref(), || {
                    shared.push(Command::RedoActive)
                });
                ui.separator();
                menu_item(ui, t!("menu.edit.copy").as_ref(), || {
                    shared.push(Command::CopyActive)
                });
            });
            menu_button(ui, t!("menu.capture").as_ref(), |ui| {
                for (tip, mode) in [
                    ("toolbar.window.tooltip", CaptureMode::WindowOrObject),
                    ("toolbar.region.tooltip", CaptureMode::Region),
                    ("toolbar.fullscreen.tooltip", CaptureMode::FullScreen),
                    ("toolbar.fixed_region.tooltip", CaptureMode::FixedRegion),
                    ("toolbar.pin.tooltip", CaptureMode::Pin),
                ] {
                    menu_item(ui, t!(tip).as_ref(), || shared.push(Command::Capture(mode)));
                }
            });
            menu_button(ui, t!("settings.title").as_ref(), |ui| {
                menu_item(ui, t!("menu.app.preferences").as_ref(), || {
                    shared.push(Command::OpenSettings)
                });
            });
        });
    }

    /// The capture tab strip: one selectable label per capture — red
    /// diamond when unsaved-or-modified, padlock when protected, a close
    /// cross, and the right-click options menu — then "+".
    /// The capture tab strip: chips in a wheel-scrollable area (no
    /// scrollbar — the reference's behaviour), overflow arrows and "+"
    /// pinned at the right edge.
    fn tab_strip(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        egui::Frame::default()
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                // egui maps the wheel onto horizontal scrolling only when
                // this flag is on; the scrollbar itself stays hidden.
                ui.style_mut().always_scroll_the_only_direction = true;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
                    // ◀ ▶ only exist while the chips overflow; max_width
                    // keeps the scroll area from pushing them off-screen.
                    let step = widgets::BUTTON_SIZE + 4.0;
                    let right_w =
                        widgets::BUTTON_SIZE + if self.tabs_overflow { 2.0 * step } else { 0.0 };
                    let strip_w = (ui.available_width() - right_w).max(60.0);
                    let mut area = egui::ScrollArea::horizontal()
                        .id_salt("pixme-tabs")
                        .auto_shrink([true, true])
                        .max_width(strip_w)
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                        );
                    if let Some(x) = self.tab_scroll_to.take() {
                        area = area.scroll_offset(egui::vec2(x, 0.0));
                    }
                    let out = area.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
                            for index in 0..self.tabs.len() {
                                self.tab_item(ui, index, shared);
                            }
                        });
                    });
                    self.tab_scroll_pos = out.state.offset.x;
                    self.scroll_active_into_view = false;
                    self.tabs_overflow = out.content_size.x > out.inner_rect.width() + 0.5;
                    if self.tabs_overflow {
                        let max_off = (out.content_size.x - out.inner_rect.width()).max(0.0);
                        ui.add_enabled_ui(self.tab_scroll_pos > 0.5, |ui| {
                            if widgets::icon_button(ui, Icon::ChevronLeft, false).clicked() {
                                self.tab_scroll_to = Some((self.tab_scroll_pos - 160.0).max(0.0));
                            }
                        });
                        ui.add_enabled_ui(self.tab_scroll_pos < max_off - 0.5, |ui| {
                            if widgets::icon_button(ui, Icon::ChevronRight, false).clicked() {
                                self.tab_scroll_to = Some(self.tab_scroll_pos + 160.0);
                            }
                        });
                    }
                    if widgets::icon_button(ui, Icon::Plus, false)
                        .on_hover_text(t!("viewer.new_capture.tooltip"))
                        .clicked()
                    {
                        shared.push(Command::ShowBar);
                    }
                });
            });
    }

    /// One tab chip: red dot when unsaved-or-modified, padlock when
    /// protected, unique title, close cross. Left/right click selects;
    /// the cross closes (never a warning — the red dot is the warning);
    /// secondary click opens the options menu at the pointer.
    fn tab_item(&mut self, ui: &mut egui::Ui, index: usize, shared: &Shared) {
        // A previous chip in this same frame may have closed tabs (the
        // cross or the context menu mutate the list mid-iteration) —
        // the index is only valid while the tab still exists.
        if index >= self.tabs.len() {
            return;
        }
        let t = theme::tokens(ui.ctx());
        let tab = &self.tabs[index];
        let (title, dirty, protected) =
            (tab.title.clone(), !tab.saved, tab.doc.meta.is_protected());
        let selected = index == self.active;
        // The cross is reserved inside the chip but registered and
        // painted AFTER the chip interact, so it sits above it and
        // keeps its own hover/click (egui: the topmost widget wins).
        let mut close_rect = Rect::NOTHING;
        let inner = egui::Frame::default()
            .inner_margin(Margin::symmetric(7, 2))
            .fill(if selected { t.accent_soft } else { t.card })
            .stroke(Stroke::new(
                1.0,
                if selected {
                    t.accent_border
                } else {
                    t.card_border
                },
            ))
            .corner_radius(CornerRadius::same(t.radius_md))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    if dirty {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                        ui.painter().circle_filled(rect.center(), 3.0, t.danger);
                    }
                    if protected {
                        let (lock_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                        icons::image(
                            Icon::Lock,
                            11.0,
                            if selected { t.accent } else { t.text_dim },
                        )
                        .paint_at(ui, lock_rect);
                    }
                    ui.add(
                        egui::Label::new(egui::RichText::new(title).color(if selected {
                            t.text
                        } else {
                            t.text_dim
                        }))
                        .selectable(false),
                    );
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                    close_rect = rect;
                })
                .inner
            });
        // Whole-chip click target (the label itself is inert) — covers
        // the padding too so clicks between markers and title count.
        let mut resp = ui.interact(
            inner.response.rect,
            ui.id().with(("pixme-tab", index)),
            Sense::click(),
        );
        // Register the cross above the chip, then paint it with the
        // resulting hover state — this is the clickable close button.
        let close = ui.interact(
            close_rect,
            ui.id().with(("pixme-tab-close", index)),
            Sense::click(),
        );
        paint_tab_close(ui, close_rect, close.hovered());
        if resp.hovered() && !selected {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if self.scroll_active_into_view && selected {
            resp.scroll_to_me(None);
        }
        resp = resp.union(close.clone());
        if close.clicked() {
            self.close_tabs(&[index]);
            return;
        }
        if (resp.clicked() || resp.secondary_clicked()) && index != self.active {
            // Selecting another tab re-locks every protected one.
            self.set_active(index);
        }
        egui::Popup::context_menu(&resp).show(|ui| {
            ui.set_min_width(200.0);
            self.tab_menu(ui, index, shared);
        });
    }

    /// The right-click options of tab `index` — the reference's menu,
    /// plus Protection (no keyboard shortcuts, per spec).
    fn tab_menu(&mut self, ui: &mut egui::Ui, index: usize, shared: &Shared) {
        let has_file = self.tabs[index].doc.meta.source_path.is_some();
        let has_saved = self.tabs.iter().any(|t| t.saved);
        let item = |ui: &mut egui::Ui, key: &str| -> Response { ui.button(t!(key).to_string()) };
        if item(ui, "viewer.tab.save").clicked() {
            if index != self.active {
                self.set_active(index);
            }
            shared.push(Command::SaveActive);
            ui.close();
        }
        if item(ui, "viewer.tab.rename").clicked() {
            if index != self.active {
                self.set_active(index);
            }
            self.open_rename();
            ui.close();
        }
        ui.add_enabled_ui(has_file, |ui| {
            if item(ui, "viewer.tab.delete_file").clicked() {
                self.confirm_delete = Some(DeleteFileDialog {
                    tab: index,
                    error: None,
                });
                ui.close();
            }
        });
        ui.separator();
        if item(ui, "viewer.tab.close").clicked() {
            self.close_tabs(&[index]);
            ui.close();
            return;
        }
        if item(ui, "viewer.tab.close_others").clicked() {
            let others: Vec<usize> = (0..self.tabs.len()).filter(|&i| i != index).collect();
            self.close_tabs(&others);
            ui.close();
            return;
        }
        ui.add_enabled_ui(has_saved, |ui| {
            if item(ui, "viewer.tab.close_saved").clicked() {
                let saved: Vec<usize> = (0..self.tabs.len())
                    .filter(|&i| self.tabs[i].saved)
                    .collect();
                self.close_tabs(&saved);
                ui.close();
            }
        });
        ui.add_enabled_ui(index > 0, |ui| {
            if item(ui, "viewer.tab.close_left").clicked() {
                self.close_tabs(&(0..index).collect::<Vec<_>>());
                ui.close();
            }
        });
        ui.add_enabled_ui(index + 1 < self.tabs.len(), |ui| {
            if item(ui, "viewer.tab.close_right").clicked() {
                self.close_tabs(&((index + 1)..self.tabs.len()).collect::<Vec<_>>());
                ui.close();
            }
        });
        if item(ui, "viewer.tab.close_all").clicked() {
            self.close_tabs(&(0..self.tabs.len()).collect::<Vec<_>>());
            ui.close();
            return;
        }
        ui.separator();
        if item(ui, "viewer.tab.duplicate").clicked() {
            self.duplicate_tab(index);
            ui.close();
        }
        // The only entry point for the screen lock — deliberately menu-
        // only (a rarely used feature keeps no dedicated chrome).
        if item(ui, "viewer.tab.protect").clicked() {
            self.protect = Some(ProtectDialog {
                tab: index,
                current: String::new(),
                new: String::new(),
                confirm: String::new(),
                error: None,
            });
            ui.close();
        }
    }

    /// The document canvas: the active capture fit to the window (or at the
    /// explicit zoom, scrollable), plus the marquee-selection interaction.
    fn content(&mut self, ui: &mut egui::Ui) {
        if self.tabs.is_empty() {
            let t = theme::tokens(ui.ctx());
            ui.centered_and_justified(|ui| {
                ui.colored_label(t.text_dim, t!("viewer.empty").to_string());
            });
            return;
        }
        let index = self.active.min(self.tabs.len() - 1);
        // A protected capture only unlocks by password — the canvas
        // (and its marquee/paste interactions) shows nothing until then.
        if self.is_locked(index) {
            self.locked_content(ui, index);
            return;
        }
        let tab = &mut self.tabs[index];
        let (img_w, img_h) = (
            tab.doc.base.image.width() as usize,
            tab.doc.base.image.height() as usize,
        );
        // The capture is in physical pixels; the viewer shows it at its
        // logical size (image px ÷ scale factor). Fit mode never upscales.
        let sf = tab.doc.base.scale_factor.max(0.25);
        if tab.texture.as_ref().map(|(rev, _)| *rev) != Some(tab.doc.revision) {
            let composited = crate::editor::render::render_to_image(&tab.doc);
            let handle = ui.ctx().load_texture(
                format!("doc-{index}-{}", tab.doc.revision),
                egui::ColorImage::from_rgba_unmultiplied([img_w, img_h], composited.as_raw()),
                egui::TextureOptions::LINEAR,
            );
            tab.texture = Some((tab.doc.revision, handle));
        }
        let tex = tab.texture.as_ref().map(|(_, h)| h.id());
        let Some(tex) = tex else { return };
        let logical = Vec2::new(img_w as f32 / sf, img_h as f32 / sf);
        let avail = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(avail, 0.0, theme::tokens(ui.ctx()).canvas);
        // Fit upscales too — a small image fills the window, matching
        // the functional reference's "fit" (not just shrink-to-fit).
        let fit = (avail.width() / logical.x.max(1.0)).min(avail.height() / logical.y.max(1.0));
        let scale = match self.zoom {
            ZoomMode::Fit => fit,
            ZoomMode::FitWidth => avail.width() / logical.x.max(1.0),
            ZoomMode::Factor(z) => z,
        };
        self.eff_scale = scale;
        let size = logical * scale;
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        egui::ScrollArea::both()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let canvas = size.max(avail.size());
                let (canvas_rect, _) = ui.allocate_exact_size(canvas, Sense::hover());
                let img_rect = Rect::from_center_size(canvas_rect.center(), size);
                ui.painter().image(tex, img_rect, uv, egui::Color32::WHITE);
            });
    }

    /// The locked tab's stand-in: canvas background plus a centred
    /// password prompt — icon, title, field, Unlock. Enter submits.
    fn locked_content(&mut self, ui: &mut egui::Ui, index: usize) {
        let t = theme::tokens(ui.ctx());
        let avail = ui.available_rect_before_wrap();
        ui.painter().rect_filled(avail, 0.0, t.canvas);
        let mut submit = false;
        egui::Area::new(egui::Id::new("pixme-unlock"))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .inner_margin(Margin::symmetric(20, 16))
                    .fill(t.card)
                    .stroke(Stroke::new(1.0, t.card_border))
                    .corner_radius(CornerRadius::same(t.radius_lg))
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            let (_, lock_rect) = ui.allocate_space(Vec2::splat(24.0));
                            icons::image(Icon::Lock, 20.0, t.accent).paint_at(ui, lock_rect);
                            ui.label(
                                egui::RichText::new(t!("unlock.title").to_string())
                                    .font(theme::semibold(14.0)),
                            );
                            ui.add_space(2.0);
                            ui.colored_label(t.text_dim, t!("unlock.hint").to_string());
                            ui.add_space(8.0);
                            let edit = egui::TextEdit::singleline(&mut self.unlock_draft)
                                .password(true)
                                .desired_width(200.0)
                                .font(theme::medium(12.0));
                            let field = ui.add(edit);
                            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                submit = true;
                            }
                            if self.unlock_error {
                                ui.add_space(4.0);
                                ui.colored_label(t.danger, t!("unlock.wrong").to_string());
                            }
                            ui.add_space(10.0);
                            if widgets::primary_button(ui, &t!("unlock.button")).clicked() {
                                submit = true;
                            }
                        });
                    });
            });
        if submit {
            if self.tabs[index].doc.meta.check_password(&self.unlock_draft) {
                self.tabs[index].unlocked = true;
                self.unlock_draft.clear();
                self.unlock_error = false;
            } else {
                self.unlock_draft.clear();
                self.unlock_error = true;
            }
        }
    }

    /// The Redimensionner window: explicit pixels only, original/new
    /// dimensions, aspect-lock linked fields — percent presets come back
    /// later through the button's menu. The draft is taken out for the
    /// frame (no borrow overlap with `self`).
    fn resize_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.resize.take() else {
            return;
        };
        let (orig_w, orig_h) = self
            .active_doc()
            .map(|d| (d.base.image.width(), d.base.image.height()))
            .unwrap_or((1, 1));
        let mut apply = false;
        let mut dismiss = false;
        let resp = egui::Modal::new(egui::Id::new("pixme-resize")).show(ctx, |ui| {
            let t = theme::tokens(ui.ctx());
            ui.set_width(320.0);
            ui.label(
                egui::RichText::new(t!("resize.title").to_string()).font(theme::semibold(14.0)),
            );
            widgets::hairline(ui);
            ui.add_space(4.0);
            // Original / new dimensions with MP count, like the
            // functional reference's dialog — the new side follows
            // the fields.
            let dims = |w: f64, h: f64| format!("{w:.0}x{h:.0}  {:.2}MP", w * h / 1e6);
            let new_dims = match (
                state.width.trim().parse::<f64>(),
                state.height.trim().parse::<f64>(),
            ) {
                (Ok(w), Ok(h)) if w > 0.0 && h > 0.0 => dims(w, h),
                _ => "—".to_owned(),
            };
            for (key, value) in [
                ("resize.original", dims(orig_w as f64, orig_h as f64)),
                ("resize.new_dim", new_dims),
            ] {
                ui.horizontal(|ui| {
                    ui.label(t!(key).to_string());
                    ui.label(egui::RichText::new(value).color(t.text_dim));
                });
            }
            ui.add_space(6.0);
            let ratio = orig_h as f64 / orig_w.max(1) as f64;
            let w_changed = ui
                .horizontal(|ui| {
                    ui.label(t!("resize.width").to_string());
                    ui.add(
                        egui::TextEdit::singleline(&mut state.width)
                            .desired_width(80.0)
                            .font(theme::medium(12.0)),
                    )
                })
                .inner
                .changed();
            let h_changed = ui
                .horizontal(|ui| {
                    ui.label(t!("resize.height").to_string());
                    ui.add(
                        egui::TextEdit::singleline(&mut state.height)
                            .desired_width(80.0)
                            .font(theme::medium(12.0)),
                    )
                })
                .inner
                .changed();
            // Editing one field rewrites the other while locked.
            if state.keep_ratio {
                if w_changed && let Ok(w) = state.width.trim().parse::<f64>() {
                    state.height = ((w * ratio).round() as u64).to_string();
                } else if h_changed && let Ok(h) = state.height.trim().parse::<f64>() {
                    state.width = ((h / ratio).round() as u64).to_string();
                }
            }
            ui.checkbox(&mut state.keep_ratio, t!("resize.keep_ratio"));
            ui.checkbox(&mut state.anti_alias, t!("resize.antialias"));
            if state.invalid {
                ui.add_space(4.0);
                ui.colored_label(t.danger, t!("resize.invalid"));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::secondary_button(ui, &t!("save.cancel")).clicked() {
                    dismiss = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_button(ui, &t!("resize.apply")).clicked() {
                        apply = true;
                    }
                });
            });
        });
        // Backdrop click or Escape = Cancel.
        if resp.should_close() {
            dismiss = true;
        }
        let mut applied = false;
        if apply {
            // Parse the draft into target pixels; invalid input only flags
            // the dialog — it never reaches the resampler.
            match (
                state.width.trim().parse::<u32>(),
                state.height.trim().parse::<u32>(),
            ) {
                (Ok(w), Ok(h)) if w > 0 && h > 0 => {
                    self.resize_active(w, h, state.anti_alias);
                    applied = true;
                }
                _ => state.invalid = true,
            }
        }
        if !(dismiss || applied) {
            self.resize = Some(state);
        }
    }

    /// The rename prompt (File → Rename / tab menu): a title field over a
    /// window. Two captures can never share a name — a taken (or empty)
    /// draft flags the dialog instead of applying.
    fn rename_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.rename.take() else {
            return;
        };
        let t = theme::tokens(ctx);
        let mut apply = false;
        let mut cancel = false;
        let resp = egui::Modal::new(egui::Id::new("pixme-rename")).show(ctx, |ui| {
            ui.set_width(300.0);
            ui.label(
                egui::RichText::new(t!("dialog.rename.title").to_string())
                    .font(theme::semibold(14.0)),
            );
            widgets::hairline(ui);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(t!("dialog.rename.label").to_string());
                let field = ui.add(
                    egui::TextEdit::singleline(&mut state.draft)
                        .desired_width(200.0)
                        .font(theme::medium(12.0)),
                );
                // Enter applies, like the Rename button.
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    apply = true;
                }
            });
            if state.taken {
                ui.add_space(4.0);
                ui.colored_label(t.danger, t!("dialog.rename.taken").to_string());
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::secondary_button(ui, &t!("save.cancel")).clicked() {
                    cancel = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_button(ui, &t!("dialog.rename.ok")).clicked() {
                        apply = true;
                    }
                });
            });
        });
        if resp.should_close() {
            cancel = true;
        }
        if apply {
            let name = state.draft.trim();
            let taken = name.is_empty() || self.title_taken(name, self.active_index());
            if taken {
                state.taken = true;
            } else {
                self.set_active_title(name.to_owned());
            }
            if !taken {
                return;
            }
        }
        if !(cancel || (apply && !state.taken)) {
            self.rename = Some(state);
        }
    }

    /// The Protection… dialog — the context menu's only entry to the
    /// screen lock. Unprotected tab: two fields (new + confirm).
    /// Protected tab: the current password must prove itself first, then
    /// a non-empty new pair re-arms, an empty new pair disarms.
    fn protect_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.protect.take() else {
            return;
        };
        let t = theme::tokens(ctx);
        let Some(tab) = self.tabs.get(state.tab) else {
            return; // the tab went away — drop the dialog
        };
        let already = tab.doc.meta.is_protected();
        let mut apply = false;
        let mut cancel = false;
        let resp = egui::Modal::new(egui::Id::new("pixme-protect")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(
                egui::RichText::new(t!("protect.title").to_string()).font(theme::semibold(14.0)),
            );
            widgets::hairline(ui);
            ui.add_space(6.0);
            let field = |ui: &mut egui::Ui, key: &str, buf: &mut String| {
                ui.horizontal(|ui| {
                    ui.label(t!(key).to_string());
                    ui.add(
                        egui::TextEdit::singleline(buf)
                            .password(true)
                            .desired_width(160.0)
                            .font(theme::medium(12.0)),
                    );
                });
            };
            if already {
                field(ui, "protect.current", &mut state.current);
            }
            field(ui, "protect.new", &mut state.new);
            field(ui, "protect.confirm", &mut state.confirm);
            if already {
                ui.add_space(4.0);
                ui.colored_label(t.text_dim, t!("protect.hint_remove").to_string());
            }
            match state.error {
                Some(ProtectError::Current) => {
                    ui.add_space(4.0);
                    ui.colored_label(t.danger, t!("protect.wrong_current").to_string());
                }
                Some(ProtectError::Mismatch) => {
                    ui.add_space(4.0);
                    ui.colored_label(t.danger, t!("protect.mismatch").to_string());
                }
                None => {}
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::secondary_button(ui, &t!("save.cancel")).clicked() {
                    cancel = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // Nothing to apply while an unprotected tab has an
                    // empty new password.
                    ui.add_enabled_ui(already || !state.new.is_empty(), |ui| {
                        if widgets::primary_button(ui, &t!("protect.apply")).clicked() {
                            apply = true;
                        }
                    });
                });
            });
        });
        if resp.should_close() {
            cancel = true;
        }
        if apply {
            state.error = None;
            if already && !self.tabs[state.tab].doc.meta.check_password(&state.current) {
                state.error = Some(ProtectError::Current);
            } else if !state.new.is_empty() && state.new != state.confirm {
                state.error = Some(ProtectError::Mismatch);
            } else {
                let tab = &mut self.tabs[state.tab];
                tab.doc.meta.set_password(state.new.trim());
                // Proving (or setting) the password unlocks for as long
                // as the tab stays active.
                tab.unlocked = true;
                self.protect = None;
                return;
            }
        }
        if !cancel {
            self.protect = Some(state);
        }
    }

    /// The "Supprimer le fichier" confirmation — deletes the tab's file
    /// on disk; the tab stays open (unsaved again) either way.
    fn delete_file_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.confirm_delete.take() else {
            return;
        };
        let t = theme::tokens(ctx);
        let Some(tab) = self.tabs.get(state.tab) else {
            return;
        };
        let name = tab
            .doc
            .meta
            .source_path
            .as_ref()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let mut confirm = false;
        let mut cancel = false;
        let resp = egui::Modal::new(egui::Id::new("pixme-delete-file")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(
                egui::RichText::new(t!("dialog.delete_file.title").to_string())
                    .font(theme::semibold(14.0)),
            );
            widgets::hairline(ui);
            ui.add_space(4.0);
            ui.label(t!("dialog.delete_file.body", name = name.as_str()).to_string());
            if let Some(err) = &state.error {
                ui.add_space(4.0);
                ui.colored_label(t.danger, err.clone());
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::secondary_button(ui, &t!("save.cancel")).clicked() {
                    cancel = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_button(ui, &t!("dialog.delete_file.confirm")).clicked() {
                        confirm = true;
                    }
                });
            });
        });
        if resp.should_close() {
            cancel = true;
        }
        if confirm {
            match self.delete_tab_file(state.tab) {
                Ok(()) => return,
                Err(e) => {
                    tracing::warn!("cannot delete file: {e}");
                    state.error = Some(e.to_string());
                }
            }
        }
        if !cancel {
            self.confirm_delete = Some(state);
        }
    }

    /// The translated Save/Discard/Cancel confirmation for a dirty session.
    fn confirm_dialog(&mut self, ctx: &egui::Context, shared: &Shared) {
        if !self.confirm_close {
            return;
        }
        let resp = egui::Modal::new(egui::Id::new("pixme-confirm")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(
                egui::RichText::new(t!("dialog.unsaved.title").to_string())
                    .font(theme::semibold(14.0)),
            );
            widgets::hairline(ui);
            ui.add_space(4.0);
            ui.label(t!("dialog.unsaved.body").to_string());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &t!("dialog.unsaved.save")).clicked() {
                    shared.push(Command::ConfirmSave);
                }
                if widgets::secondary_button(ui, &t!("dialog.unsaved.discard")).clicked() {
                    shared.push(Command::ConfirmDiscard);
                }
                if widgets::secondary_button(ui, &t!("dialog.unsaved.cancel")).clicked() {
                    shared.push(Command::ConfirmCancel);
                }
            });
        });
        // A backdrop click is ambiguous here — only explicit buttons
        // resolve the save/discard/cancel choice, so Esc/backdrop maps
        // to Cancel.
        if resp.should_close() {
            shared.push(Command::ConfirmCancel);
        }
    }
}

/// The tab's close cross: a transparent square, hover-filled, the X
/// tinted like an icon (danger on hover) with a pointing-hand cursor.
/// Paints the tab's close cross inside `rect` — the hovered state
/// comes from the interact registered above the chip (see `tab_item`).
fn paint_tab_close(ui: &mut egui::Ui, rect: Rect, hovered: bool) {
    let t = theme::tokens(ui.ctx());
    if ui.is_rect_visible(rect) {
        if hovered {
            // A red soft fill + outline makes the hover unmistakable.
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
}

/// One dropdown popup anchored to a tool button. Must be called EVERY
/// frame — `Popup::menu` only renders while its id is open in memory,
/// so gating the call on the click frame would flash it for a single
/// frame. `open` forces it open on the caret-click frame instead.
fn tool_popup(
    ui: &egui::Ui,
    resp: &Response,
    salt: &'static str,
    open: bool,
    add: impl FnOnce(&mut egui::Ui),
) {
    egui::Popup::menu(resp)
        // Explicit id: our buttons allocate with auto ids, so three
        // adjacent ones would share the default popup id.
        .id(ui.id().with(salt))
        .open_memory(open.then_some(egui::SetOpenCommand::Bool(true)))
        .show(|ui| {
            ui.set_min_width(200.0);
            add(ui);
        });
}

/// Ouvrir's menu: New / Open / Import from clipboard / last used folder
/// (greyed until a folder has been used once).
fn open_menu(ui: &egui::Ui, resp: &Response, shared: &Shared, has_last_dir: bool, open: bool) {
    tool_popup(ui, resp, "open-menu", open, |ui| {
        for (key, cmd) in [
            ("vbar.open.new", Command::NewDocument),
            ("menu.file.open", Command::OpenFile),
            ("menu.app.import_clipboard", Command::ImportClipboard),
        ] {
            if ui.button(t!(key)).clicked() {
                shared.push(cmd);
                ui.close();
            }
        }
        ui.separator();
        ui.add_enabled_ui(has_last_dir, |ui| {
            if ui.button(t!("vbar.open.last_dir")).clicked() {
                shared.push(Command::OpenLastDir);
                ui.close();
            }
        });
    });
}

/// The zoom menu: actual size, fit window/width, then the 100→500%
/// presets the functional reference lists.
fn zoom_menu(ui: &egui::Ui, resp: &Response, v: &mut Viewer, open: bool) {
    tool_popup(ui, resp, "zoom-menu", open, |ui| {
        for (mode, key) in [
            (ZoomMode::Factor(1.0), "zoom.actual"),
            (ZoomMode::Fit, "zoom.fit_window"),
            (ZoomMode::FitWidth, "zoom.fit_width"),
        ] {
            if ui.selectable_label(v.zoom == mode, t!(key)).clicked() {
                v.zoom = mode;
                ui.close();
            }
        }
        ui.separator();
        for pct in [100u32, 200, 300, 400, 500] {
            let mode = ZoomMode::Factor(pct as f32 / 100.0);
            if ui
                .selectable_label(v.zoom == mode, format!("{pct}%"))
                .clicked()
            {
                v.zoom = mode;
                ui.close();
            }
        }
    });
}

/// One top-level menu label opening a popup (egui menu strip style).
fn menu_button(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    let resp = ui.button(label);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(180.0);
        add(ui);
    });
}

/// One menu entry dispatching a [`Command`] when clicked.
fn menu_item(ui: &mut egui::Ui, label: &str, action: impl FnOnce()) {
    if ui.button(label).clicked() {
        action();
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Capture, MonitorInfo, PhysicalRect};

    /// A tab backed by an `n × n` opaque document.
    fn push_doc(v: &mut Viewer, w: u32, h: u32) {
        v.tabs.push(Tab {
            title: "t".to_owned(),
            doc: Document::new(Capture {
                image: image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255])),
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
            }),
            saved: false,
            texture: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            anno_undo: VecDeque::new(),
            anno_redo: VecDeque::new(),
            unlocked: false,
        });
        v.active = v.tabs.len() - 1;
    }

    #[test]
    fn copy_returns_the_whole_document() {
        let mut v = Viewer::default();
        push_doc(&mut v, 10, 10);
        let img = v.copy_image().expect("copy");
        assert_eq!((img.width(), img.height()), (10, 10));
        assert_eq!(img.get_pixel(0, 0).0, [10, 20, 30, 255]);
    }

    #[test]
    fn undo_redo_resize_restores_dimensions() {
        let mut v = Viewer::default();
        push_doc(&mut v, 20, 20);
        v.resize_active(8, 8, true);
        let dims = |v: &Viewer| {
            (
                v.tabs[0].doc.base.image.width(),
                v.tabs[0].doc.base.image.height(),
            )
        };
        assert_eq!(dims(&v), (8, 8));
        assert!(v.undo_active());
        assert_eq!(dims(&v), (20, 20));
        assert!(v.redo_active());
        assert_eq!(dims(&v), (8, 8));
    }

    #[test]
    fn titles_stay_unique_on_push_and_duplicate() {
        let mut v = Viewer::default();
        v.push_stub("shot");
        v.push_stub("shot");
        assert_eq!(v.tabs[0].title, "shot");
        assert_eq!(v.tabs[1].title, "shot (2)");
        v.duplicate_tab(0);
        assert_eq!(v.tabs[2].title, "shot (3)");
    }

    #[test]
    fn title_taken_is_case_insensitive_and_self_exempt() {
        let mut v = Viewer::default();
        v.push_stub("Shot");
        v.push_stub("other");
        assert!(v.title_taken("shot", None));
        assert!(v.title_taken("  SHOT ", None));
        assert!(
            !v.title_taken("shot", Some(0)),
            "a tab may keep its own name"
        );
    }

    #[test]
    fn close_left_right_others_track_active() {
        let mut v = Viewer::default();
        for t in ["a", "b", "c", "d", "e"] {
            v.push_stub(t);
        }
        v.set_active(3); // "d"
        v.close_tabs(&[4]); // close right of active
        assert_eq!(v.active, 3);
        v.close_tabs(&[0, 1]); // close left
        assert_eq!(v.active, 1, "active shifts left over removed tabs");
        assert_eq!(v.tabs[v.active].title, "d");
        // Close others: only the active tab remains.
        let others: Vec<usize> = (0..v.tabs.len()).filter(|&i| i != v.active).collect();
        v.close_tabs(&others);
        assert_eq!(v.tabs.len(), 1);
        assert_eq!(v.tabs[0].title, "d");
        // Closing the active tab clamps and resets the view.
        v.close_tabs(&[0]);
        assert!(v.tabs.is_empty());
        assert_eq!(v.active, 0);
    }

    #[test]
    fn close_saved_only_drops_saved_tabs() {
        let mut v = Viewer::default();
        for t in ["a", "b", "c"] {
            v.push_stub(t);
        }
        v.mark_saved(1, std::path::PathBuf::from("/tmp/pixme-test.png"));
        assert!(v.tabs[1].doc.meta.saved_at.is_some());
        assert_eq!(
            v.tabs[1].doc.meta.source_path.as_deref(),
            Some(std::path::Path::new("/tmp/pixme-test.png"))
        );
        let saved: Vec<usize> = (0..v.tabs.len()).filter(|&i| v.tabs[i].saved).collect();
        v.close_tabs(&saved);
        assert_eq!(v.tabs.len(), 2);
    }

    #[test]
    fn duplicate_keeps_protection_but_gets_fresh_identity() {
        let mut v = Viewer::default();
        v.push_stub("a");
        v.tabs[0].doc.meta.set_password("pw");
        v.tabs[0].doc.meta.saved_at = Some(chrono::Local::now());
        v.duplicate_tab(0);
        let copy = &v.tabs[1];
        assert_ne!(copy.doc.meta.id, v.tabs[0].doc.meta.id);
        assert!(copy.doc.meta.is_protected(), "a copy stays protected");
        assert!(copy.doc.meta.saved_at.is_none(), "the copy is unsaved");
        assert!(copy.doc.meta.source_path.is_none());
    }

    #[test]
    fn protected_tab_relocks_on_switch() {
        let mut v = Viewer::default();
        v.push_stub("a");
        v.push_stub("b");
        v.tabs[0].doc.meta.set_password("pw");
        v.set_active(0);
        assert!(v.is_locked(0));
        // Right password unlocks while the tab stays active.
        assert!(v.tabs[0].doc.meta.check_password("pw"));
        v.tabs[0].unlocked = true;
        assert!(!v.is_locked(0));
        // Leaving the tab re-locks it.
        v.set_active(1);
        assert!(v.is_locked(0));
        assert!(!v.is_locked(1));
    }

    #[test]
    fn delete_tab_file_unlinks_and_marks_unsaved() {
        let path = std::env::temp_dir().join("pixme-delete-test.png");
        std::fs::write(&path, b"png").expect("fixture");
        let mut v = Viewer::default();
        v.push_stub("a");
        v.mark_saved(0, path.clone());
        assert!(v.tabs[0].saved);
        v.delete_tab_file(0).expect("delete");
        assert!(!path.exists());
        assert!(!v.tabs[0].saved);
        assert!(v.tabs[0].doc.meta.source_path.is_none());
        assert!(v.dirty);
    }
}
