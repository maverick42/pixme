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
use crate::config::TabSort;
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
    /// `(revision, thumbnail)` cache for the mosaic grid — the
    /// composited document downscaled to ≤ [`THUMB_MAX_SIDE`] px on its
    /// longest side, built lazily only while the tile is visible. The
    /// full-resolution `texture` above is never involved, and a locked
    /// tab never holds one (the pixels stay hidden).
    thumb: Option<(u64, egui::TextureHandle)>,
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
    /// Insertion-order key — survives re-sorting so "insertion order"
    /// can always be restored.
    pub insertion: usize,
    /// Password accepted while this tab is visible — leaving the tab
    /// re-locks it (`false` again), so a protected capture asks for its
    /// password on every activation.
    pub unlocked: bool,
    /// `doc.revision` as the store last saw it (`Some` on restored
    /// tabs, `None` on new ones) — unchanged tabs skip `png_encode` +
    /// postcard at quit.
    pub persisted_rev: Option<u64>,
    /// True when `doc.base.image` is not yet in the store (new tab, or
    /// mutated by flatten/resize/undo). Annotation-only edits leave it
    /// `false` so quit updates the row without touching `base_png`.
    pub base_dirty: bool,
}

/// How many pixel mutations stay undoable per tab. Full-image ops keep
/// one pre-image each (~20 MB on a Retina capture), so the cap bounds
/// the worst case.
const HISTORY_CAP: usize = 10;

/// Longest side of a mosaic thumbnail, px — the lazy per-tab texture
/// budget for the grid (~0.9 MB each at the cap).
const THUMB_MAX_SIDE: u32 = 480;

/// Height of a mosaic tile's caption band, pt.
const BAND_H: f32 = 22.0;

/// What the save-as dialog needs to write a viewer document.
pub struct SaveTarget {
    /// Tab index (marked clean by `mark_saved` on success).
    pub index: usize,
    /// Tab title — seeds the file name for never-saved documents.
    pub title: String,
    /// The composited document as a standalone capture.
    pub capture: crate::capture::Capture,
    /// File the document was loaded/saved from — overwriting it is
    /// allowed (re-saving IS the point for that file).
    pub source: Option<std::path::PathBuf>,
}

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
/// What the resize dialog's width/height fields express.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeUnit {
    /// Target size in physical pixels.
    Pixels,
    /// Target size as a percentage of the original.
    Percent,
}

struct ResizeDialog {
    /// Draft width (px or % of the original, per `unit`).
    width: String,
    /// Draft height (px or % of the original, per `unit`).
    height: String,
    /// The fields' unit.
    unit: ResizeUnit,
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

/// How the viewer arranges its documents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ViewMode {
    /// One active document with the tab strip on top.
    #[default]
    Tabs,
    /// A grid of thumbnails; the tab strip is hidden.
    Mosaic,
}

/// Session state rendered inside the viewer viewport.
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
    /// Running insertion counter — the "insertion order" sort key.
    next_insertion: usize,
    /// Current tab ordering (persisted via `settings.tab_sort`).
    sort_mode: Option<TabSort>,
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
    /// Save-all found two unsaved captures sharing a name — show the
    /// blocking notice until dismissed.
    pub dup_names: bool,
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
    /// Internal ids of documents whose tabs were closed since the last
    /// drain — the app forwards them to the session store so their
    /// rows never come back at the next launch.
    closed_ids: Vec<String>,
    /// The persisted session was already loaded once — restore never
    /// runs twice (a deliberate discard stays discarded).
    restored: bool,
    /// Some(start) while the session loads on its worker thread — the
    /// spinner only shows past ~80 ms so small sessions never flash.
    loading_since: Option<std::time::Instant>,
    /// `(content_sig, meta_sig)` of the session as the store last saw
    /// it (set at restore and after each write). Quit compares to skip
    /// a pointless full rewrite — or downgrade it to a meta update.
    pub persisted_sigs: Option<(u64, u64)>,
    /// Tab strip vs thumbnail grid — a runtime-only choice: PixMe
    /// always opens in Tabs mode.
    view_mode: ViewMode,
    /// Mosaic thumbnail scale (0.5–2.0; 1.0 = the base 220×160 tiles).
    mosaic_scale: f32,
}

impl Default for Viewer {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            dirty: false,
            confirm_close: false,
            confirm_then_quit: false,
            next_title: 0,
            next_insertion: 0,
            sort_mode: None,
            zoom: ZoomMode::default(),
            eff_scale: 0.0,
            resize: None,
            dup_names: false,
            rename: None,
            protect: None,
            confirm_delete: None,
            unlock_draft: String::new(),
            unlock_error: false,
            tab_scroll_pos: 0.0,
            tab_scroll_to: None,
            scroll_active_into_view: false,
            tabs_overflow: false,
            tool_scroll_pos: 0.0,
            tool_scroll_to: None,
            tools_overflow: false,
            closed_ids: Vec::new(),
            restored: false,
            loading_since: None,
            persisted_sigs: None,
            view_mode: ViewMode::Tabs,
            mosaic_scale: 1.0,
        }
    }
}

/// The rename prompt: draft + whether it already failed uniqueness.
struct RenameState {
    /// Field content.
    draft: String,
    /// Last Apply hit a duplicate/empty name — inline error shown.
    taken: bool,
    /// How many of the next tabs (current order) also get renamed —
    /// when > 0 the sequence covers the current tab too (`{draft} - 1`
    /// on it). Clamped to the following-tab count on use.
    follow: usize,
    /// The name field grabs focus once — re-requesting every frame
    /// would steal it back from the follow field.
    focused: bool,
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
        self.base_dirty = true;
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
        let insertion = self.next_insertion;
        self.next_insertion += 1;
        self.tabs.push(Tab {
            title,
            doc,
            saved,
            texture: None,
            thumb: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            anno_undo: VecDeque::new(),
            anno_redo: VecDeque::new(),
            unlocked: false,
            insertion,
            persisted_rev: None,
            base_dirty: true,
        });
        // A new document gets a fresh view — a stale zoom would
        // leak onto unrelated pixels — and the tab we leave re-locks.
        let new = self.tabs.len() - 1;
        self.set_active(new);
        // Sorting may move the new tab; set_active ran first so
        // sort_tabs remaps `active` by document id.
        self.sort_tabs();
        self.recompute_dirty();
    }

    /// Adds `doc` as a fresh tab with `title` VERBATIM — the capture
    /// naming template may legitimately repeat (duplicates are caught at
    /// save-all, where a rename is the fix).
    pub fn push_capture_document(&mut self, doc: Document, title: String) {
        let insertion = self.next_insertion;
        self.next_insertion += 1;
        self.tabs.push(Tab {
            title,
            doc,
            saved: false,
            texture: None,
            thumb: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            anno_undo: VecDeque::new(),
            anno_redo: VecDeque::new(),
            unlocked: false,
            insertion,
            persisted_rev: None,
            base_dirty: true,
        });
        let new = self.tabs.len() - 1;
        self.set_active(new);
        self.sort_tabs();
        self.recompute_dirty();
    }

    /// Test helper: a tab whose document wraps a 1×1 capture (goes
    /// through the unique-title rule like the real push paths).
    #[cfg(test)]
    pub fn push_stub(&mut self, title: &str) {
        use crate::capture::{Capture, MonitorInfo, PhysicalRect};
        let title = self.unique_title(title);
        let insertion = self.next_insertion;
        self.next_insertion += 1;
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
            thumb: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            anno_undo: VecDeque::new(),
            anno_redo: VecDeque::new(),
            unlocked: false,
            insertion,
            persisted_rev: None,
            base_dirty: true,
        });
    }

    /// The session's dirty flag = any tab not written to disk.
    pub fn recompute_dirty(&mut self) {
        self.dirty = self.tabs.iter().any(|t| !t.saved);
    }

    /// Marks `index` saved at `path` and refreshes the session flag —
    /// called after a save-as dialog completes for a viewer document.
    /// The meta records the location and the timestamp ("was saved at
    /// least once, and when") for the future database restore. The tab
    /// takes the written file's stem: a save-as under another name
    /// renames the document with it (like File → Open does).
    pub fn mark_saved(&mut self, index: usize, path: std::path::PathBuf) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.saved = true;
            if let Some(stem) = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
            {
                tab.title = stem;
            }
            tab.doc.meta.source_path = Some(path);
            tab.doc.meta.saved_at = Some(chrono::Local::now());
            // A title change is a sort key under ByName — re-sort.
            self.sort_tabs();
            self.recompute_dirty();
        }
    }

    /// Index of the active tab, clamped into range (`None` when empty).
    pub fn active_index(&self) -> Option<usize> {
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
    /// the tab's — objects stay re-editable on the next entry. A no-op
    /// commit (nothing changed since Draw opened) leaves the tab clean.
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
        let doc_changed = postcard::to_allocvec(&t.doc.annotations).ok()
            != postcard::to_allocvec(&doc.annotations).ok();
        let hist_changed = postcard::to_allocvec(&t.anno_undo).ok()
            != postcard::to_allocvec(&undo).ok()
            || postcard::to_allocvec(&t.anno_redo).ok() != postcard::to_allocvec(&redo).ok();
        if !doc_changed && !hist_changed {
            return;
        }
        t.doc.set_next_id(doc.next_id());
        t.anno_undo = undo;
        t.anno_redo = redo;
        if doc_changed {
            t.doc = doc;
            t.doc.revision += 1;
            // New annotations sit on top of the current base image —
            // reverting an older PixelOp (Resize) under them would
            // desynchronise coordinates, so the pixel history ends here.
            t.undo.clear();
            t.redo.clear();
            t.saved = false;
            self.dirty = true;
        }
    }

    /// Everything the save-as dialog needs for the active tab.
    pub fn save_target(&self) -> Option<SaveTarget> {
        self.save_target_at(self.active_index()?)
    }

    /// Same as [`Self::save_target`] for an explicit tab — the
    /// close-request confirmation saves a tab that is not the active one.
    pub fn save_target_at(&self, index: usize) -> Option<SaveTarget> {
        let tab = self.tabs.get(index)?;
        let mut capture = tab.doc.base.clone();
        capture.image = crate::editor::render::render_to_image(&tab.doc);
        Some(SaveTarget {
            index,
            title: tab.title.clone(),
            capture,
            source: tab.doc.meta.source_path.clone(),
        })
    }

    /// Opens the rename prompt on the active tab (File → Rename).
    pub fn open_rename(&mut self) {
        if let Some(title) = self.active_title() {
            self.rename = Some(RenameState {
                draft: title,
                taken: false,
                follow: 0,
                focused: false,
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

    /// Renames the active tab (the File → Rename command). A rename is
    /// a sort key under `ByName` — re-sort right away.
    pub fn set_active_title(&mut self, title: String) {
        self.apply_rename(title, 0);
    }

    /// Rename-dialog apply: the active tab takes `name`; the `follow`
    /// tabs that come after it in the current order get `{name} - k`,
    /// snapshotted by document id BEFORE the sort renames can trigger.
    /// A renamed tab becomes unsaved: its title no longer matches the
    /// file on disk (`source_path` is kept so Enregistrer still
    /// proposes that folder).
    fn apply_rename(&mut self, name: String, follow: usize) {
        let Some(ai) = self.active_index() else {
            return;
        };
        let ids: Vec<_> = self.tabs[ai + 1..]
            .iter()
            .take(follow)
            .map(|t| t.doc.meta.id.clone())
            .collect();
        // follow > 0 starts the numbered series on the current tab:
        // `{name} - 1` on it, ` - 2`, ` - 3`… on the next ones.
        let first = if follow > 0 {
            format!("{name} - 1")
        } else {
            name.clone()
        };
        if self.tabs[ai].title != first {
            self.tabs[ai].title = first;
            self.tabs[ai].saved = false;
        }
        for (k, id) in ids.iter().enumerate() {
            if let Some(tab) = self.tabs.iter_mut().find(|t| t.doc.meta.id == *id) {
                let next = format!("{name} - {}", k + 2);
                if tab.title != next {
                    tab.title = next;
                    tab.saved = false;
                }
            }
        }
        self.recompute_dirty();
        self.sort_tabs();
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
        tab.base_dirty = true;
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
        tab.base_dirty = true;
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
        tab.base_dirty = true;
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
    /// once per session (the spec's rule). Re-locking also drops the
    /// tab's cached thumbnail — a protected tab never keeps pixels.
    fn set_active(&mut self, index: usize) {
        self.active = index;
        for tab in &mut self.tabs {
            tab.unlocked = false;
            if tab.doc.meta.is_protected() {
                tab.thumb = None;
            }
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
                // Remember the doc id: its stored row is deleted next
                // frame so the closed capture stays closed.
                let removed = self.tabs.remove(i);
                self.closed_ids.push(removed.doc.meta.id);
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

    /// Doc ids of tabs closed since the last call — the app forwards
    /// them to the store so the rows are deleted.
    pub fn take_closed_ids(&mut self) -> Vec<String> {
        std::mem::take(&mut self.closed_ids)
    }

    /// `true` once the persisted session has been loaded — restore is
    /// a one-shot, never retried within a run.
    pub fn session_restored(&self) -> bool {
        self.restored
    }

    /// Claims the one-shot restore: a loader thread was spawned, the
    /// session lands via `restore_session` when it finishes.
    pub fn start_restore(&mut self) {
        self.restored = true;
        self.loading_since = Some(std::time::Instant::now());
    }

    /// The auto-title counter — persisted so `Capture N` keeps counting
    /// across restarts instead of colliding with restored names.
    pub fn next_title(&self) -> usize {
        self.next_title as usize
    }

    /// Replaces the (empty) session with what the store kept: every tab
    /// comes back with its document, title, saved flag and annotation
    /// history; protected tabs restore LOCKED (`unlocked: false`, the
    /// password is asked on activation like any tab switch).
    pub fn restore_session(
        &mut self,
        session: crate::store::StoredSession,
        sort_mode: Option<TabSort>,
    ) {
        self.restored = true;
        self.loading_since = None;
        self.sort_mode = sort_mode;
        let had_live_tabs = !self.tabs.is_empty();
        // Baseline for the quit-time change check: what the store
        // holds is exactly this session only when no new tab arrived
        // while the loader thread was running.
        if session.tabs.is_empty() {
            self.persisted_sigs = (!had_live_tabs).then(|| self.sigs());
            return;
        }
        self.next_title = self.next_title.max(session.next_title as u32);
        let restored_count = session.tabs.len();
        if had_live_tabs {
            for t in &mut self.tabs {
                t.insertion += restored_count;
            }
        }
        let mut restored_tabs = Vec::with_capacity(restored_count + self.tabs.len());
        for (i, t) in session.tabs.into_iter().enumerate() {
            let persisted_rev = Some(t.doc.revision);
            restored_tabs.push(Tab {
                title: t.title,
                doc: t.doc,
                saved: t.saved,
                texture: None,
                thumb: None,
                undo: VecDeque::new(),
                redo: VecDeque::new(),
                anno_undo: t.anno_undo,
                anno_redo: t.anno_redo,
                unlocked: false,
                insertion: i,
                persisted_rev,
                base_dirty: false,
            });
        }
        self.next_insertion += restored_count;
        restored_tabs.append(&mut self.tabs);
        self.tabs = restored_tabs;
        if had_live_tabs {
            self.active = (self.active + restored_count).min(self.tabs.len() - 1);
        } else if !self.tabs.is_empty() {
            self.set_active(session.active.min(self.tabs.len() - 1));
        }
        self.sort_tabs();
        self.recompute_dirty();
        self.persisted_sigs = (!had_live_tabs).then(|| self.sigs());
    }

    /// Cheap fingerprints of what `save_session` persists, hashed in
    /// tab order (position = the stored `ord`): `content` covers every
    /// per-tab field — mutations bump `doc.revision`, the rest is
    /// hashed directly — `meta` adds the session row (active tab,
    /// title counter). Equal content ⇒ the rows are already written.
    pub fn sigs(&self) -> (u64, u64) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut content = DefaultHasher::new();
        for tab in &self.tabs {
            let d = &tab.doc;
            d.meta.id.hash(&mut content);
            tab.title.hash(&mut content);
            tab.saved.hash(&mut content);
            d.revision.hash(&mut content);
            d.next_id().hash(&mut content);
            d.meta.captured_at.timestamp().hash(&mut content);
            d.meta.saved_at.map(|s| s.timestamp()).hash(&mut content);
            d.meta.source_path.hash(&mut content);
            d.meta.password_hash.hash(&mut content);
        }
        let mut meta = content.clone();
        self.active.hash(&mut meta);
        self.next_title.hash(&mut meta);
        (content.finish(), meta.finish())
    }

    /// Reorders tabs per `sort_mode` (`None` = insertion order). The
    /// active tab keeps its selection — remapped by document id since
    /// sorting moves indices.
    fn sort_tabs(&mut self) {
        let active_id = self
            .active_index()
            .map(|i| self.tabs[i].doc.meta.id.clone());
        match self.sort_mode {
            Some(TabSort::ByName) => self.tabs.sort_by(|a, b| {
                natural_cmp(&a.title, &b.title).then(a.insertion.cmp(&b.insertion))
            }),
            Some(TabSort::ByDate) => self
                .tabs
                .sort_by_key(|t| (t.doc.meta.captured_at, t.insertion)),
            None => self.tabs.sort_by_key(|t| t.insertion),
        }
        if let Some(id) = active_id
            && let Some(pos) = self.tabs.iter().position(|t| t.doc.meta.id == id)
        {
            self.active = pos;
        }
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
            thumb: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            anno_undo: src.anno_undo.clone(),
            anno_redo: src.anno_redo.clone(),
            unlocked: false,
            insertion: self.next_insertion,
            persisted_rev: None,
            base_dirty: true,
        });
        self.next_insertion += 1;
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

    /// Toggles between the tab strip and the mosaic grid. Runtime-only:
    /// PixMe always opens in Tabs mode.
    fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Tabs => ViewMode::Mosaic,
            ViewMode::Mosaic => ViewMode::Tabs,
        };
    }

    /// Mosaic zoom in: ×1.25, clamped to 200%.
    fn mosaic_zoom_in(&mut self) {
        self.mosaic_scale = (self.mosaic_scale * 1.25).min(2.0);
    }

    /// Mosaic zoom out: ÷1.25, clamped to 50%.
    fn mosaic_zoom_out(&mut self) {
        self.mosaic_scale = (self.mosaic_scale / 1.25).max(0.5);
    }

    /// Mosaic zoom back to 100% (the percent button's main click).
    fn mosaic_zoom_reset(&mut self) {
        self.mosaic_scale = 1.0;
    }

    /// The mosaic's double-click/Enter action: make `index` the active
    /// tile (re-locking protected tabs when the selection changes, like
    /// the tab strip) and hand the viewer back to Tabs mode — a locked
    /// capture then shows its usual password card.
    fn open_active_tile(&mut self, index: usize) {
        if index != self.active {
            self.set_active(index);
        }
        self.view_mode = ViewMode::Tabs;
    }

    /// Lazily builds `index`'s downscaled thumbnail: the composited
    /// document is resampled so its longest side is ≤
    /// [`THUMB_MAX_SIDE`] px and cached on the tab keyed by the
    /// document revision. Locked tabs never get one — their pixels
    /// stay hidden — and the full-resolution `Tab::texture` is never
    /// involved.
    fn ensure_thumb(&mut self, ui: &egui::Ui, index: usize) {
        // Protected tabs never generate pixel data — locked or not
        // (the "proven password" only unlocks the canvas, the mosaic
        // tile stays a padlock).
        let protected = self
            .tabs
            .get(index)
            .is_some_and(|t| t.doc.meta.is_protected());
        if self.is_locked(index) || protected {
            return;
        }
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        if tab.thumb.as_ref().map(|(rev, _)| *rev) == Some(tab.doc.revision) {
            return;
        }
        let (w, h) = (tab.doc.base.image.width(), tab.doc.base.image.height());
        let composited = crate::editor::render::render_to_image(&tab.doc);
        let scale = (THUMB_MAX_SIDE as f32 / w.max(h).max(1) as f32).min(1.0);
        let (tw, th) = (
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
        );
        let thumb =
            image::imageops::resize(&composited, tw, th, image::imageops::FilterType::Lanczos3);
        let handle = ui.ctx().load_texture(
            format!("thumb-{index}-{}", tab.doc.revision),
            egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], thumb.as_raw()),
            egui::TextureOptions::LINEAR,
        );
        tab.thumb = Some((tab.doc.revision, handle));
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
            || self.dup_names
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
                // No default vertical gap between strips — every strip
                // carries its own margins, so the column stays flush
                // and compact like the reference.
                ui.spacing_mut().item_spacing.y = 0.0;
                if !shared.has_native_app_menu() {
                    self.menu_strip(ui, shared);
                    widgets::hairline(ui);
                }
                self.toolbar_strip(ui, shared);
                widgets::hairline(ui);
                if self.view_mode == ViewMode::Tabs {
                    self.tab_strip(ui, shared);
                    widgets::hairline(ui);
                }
                self.content(ui, shared);
                // Session restore in flight: the window is already up,
                // the spinner waits ~80 ms so a small session never
                // flashes.
                if let Some(since) = self.loading_since
                    && since.elapsed() > std::time::Duration::from_millis(80)
                {
                    paint_loading(ui);
                    ui.ctx().request_repaint();
                }
                // Modal behaviour for child windows living in other
                // viewports (save-as, settings): dim the whole panel and
                // eat every click while one is up.
                if shared.modal_child_open() {
                    let rect = ui.max_rect();
                    ui.painter()
                        .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(70));
                    ui.allocate_rect(rect, Sense::click_and_drag());
                }
                // Quit-time save on its worker thread: an opaque veil
                // (the tabs moved out), the spinner and a message
                // cover everything until pump() closes the app.
                if shared.saving() {
                    let rect = ui.max_rect();
                    ui.painter().rect_filled(rect, 0.0, t.dock);
                    paint_loading(ui);
                    ui.painter().text(
                        rect.center() + Vec2::new(0.0, 36.0),
                        egui::Align2::CENTER_CENTER,
                        t!("quit.saving").as_ref(),
                        theme::medium(12.0),
                        t.text,
                    );
                    ui.allocate_rect(rect, Sense::click_and_drag());
                    ctx.request_repaint();
                } else if let Some((done, total)) = shared.save_all_progress() {
                    // "Tout enregistrer" in flight: same opaque veil,
                    // plus the written/total counter and a progress bar.
                    let rect = ui.max_rect();
                    ui.painter().rect_filled(rect, 0.0, t.dock);
                    paint_loading(ui);
                    ui.painter().text(
                        rect.center() + Vec2::new(0.0, 36.0),
                        egui::Align2::CENTER_CENTER,
                        t!("viewer.saving_all", done = done, total = total).as_ref(),
                        theme::medium(12.0),
                        t.text,
                    );
                    let bar = Rect::from_center_size(
                        rect.center() + Vec2::new(0.0, 60.0),
                        Vec2::new(240.0, 6.0),
                    );
                    ui.put(
                        bar,
                        egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                            .desired_height(6.0)
                            .corner_radius(3.0),
                    );
                    ui.allocate_rect(rect, Sense::click_and_drag());
                    ctx.request_repaint();
                }
            });
        self.confirm_dialog(&ctx, shared);
        self.resize_dialog(&ctx);
        self.rename_dialog(&ctx);
        self.protect_dialog(&ctx);
        self.delete_file_dialog(&ctx);
        self.dup_names_dialog(&ctx);
        self.settings_window(&ctx, shared);
    }

    /// The settings window is modal to the viewer: a centred custom
    /// panel over the dimmed content (`modal_child_open` eats the
    /// clicks). Its header reuses the tab's painted cross —
    /// [`paint_tab_close`] — instead of egui's native title bar.
    fn settings_window(&self, ctx: &egui::Context, shared: &Shared) {
        if !shared
            .settings_open
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let mut open = true;
        egui::Area::new(egui::Id::new("pixme.settings_modal"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                let t = theme::tokens(ui.ctx());
                egui::Frame::default()
                    .fill(t.dock)
                    .stroke(Stroke::new(1.0, t.card_border))
                    .corner_radius(CornerRadius::same(t.radius_lg))
                    .show(ui, |ui| {
                        // Fixed width: inside an Area `available_width`
                        // is the whole window — rows would stretch the
                        // modal to full size without this bound. Wider
                        // than the old single-column sheet: sidebar +
                        // page + footer.
                        ui.set_width(560.0);
                        // Flush stack: the band edge separates header
                        // from body — no stray gaps from item spacing.
                        ui.spacing_mut().item_spacing.y = 0.0;
                        // Title-bar band: card tint rounded on its top
                        // corners — reads as a header while keeping our
                        // painted cross (same look on Windows: nothing
                        // here is native chrome).
                        egui::Frame::default()
                            .inner_margin(Margin::symmetric(10, 6))
                            .fill(t.card)
                            .corner_radius(CornerRadius {
                                nw: t.radius_lg,
                                ne: t.radius_lg,
                                sw: 0,
                                se: 0,
                            })
                            .show(ui, |ui| {
                                ui.set_width(560.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(t!("settings.title").to_string())
                                            .color(t.text)
                                            .strong(),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let (rect, _) = ui.allocate_exact_size(
                                                Vec2::splat(20.0),
                                                Sense::hover(),
                                            );
                                            // Painted above like the tab
                                            // chip: the close cross keeps
                                            // its own hover and click.
                                            let resp = ui.interact(
                                                rect,
                                                ui.id().with("settings-close"),
                                                Sense::click(),
                                            );
                                            paint_tab_close(ui, rect, resp.hovered());
                                            if resp.clicked() {
                                                open = false;
                                            }
                                        },
                                    );
                                });
                            });
                        egui::Frame::default()
                            .inner_margin(Margin::symmetric(14, 0))
                            .show(ui, |ui| crate::ui::settings::show(ui, shared));
                    });
            });
        if !open {
            shared
                .settings_open
                .store(false, std::sync::atomic::Ordering::Relaxed);
            // Closing via the cross discards the draft like Annuler.
            *crate::app::lock(&shared.settings_draft) = None;
        }
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
            if let Some(index) = self.active_index() {
                // Capture date in the active language's local format,
                // then the current sort mode after a bullet separator.
                let fmt = capture_date_fmt();
                let date = self.tabs[index].doc.meta.captured_at.format(fmt);
                let mode = match self.sort_mode {
                    Some(TabSort::ByName) => t!("viewer.tab.sort_name"),
                    Some(TabSort::ByDate) => t!("viewer.tab.sort_date"),
                    None => t!("viewer.sort.insertion"),
                };
                ui.label(
                    egui::RichText::new(format!("{date} · {mode}"))
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
                                if self.view_mode == ViewMode::Mosaic {
                                    self.mosaic_zoom_in();
                                } else {
                                    self.zoom =
                                        ZoomMode::Factor((self.scale_for_step() * 1.25).min(8.0));
                                }
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
                                if self.view_mode == ViewMode::Mosaic {
                                    self.mosaic_zoom_out();
                                } else {
                                    self.zoom =
                                        ZoomMode::Factor((self.scale_for_step() / 1.25).max(0.05));
                                }
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
                                if self.view_mode == ViewMode::Mosaic {
                                    self.mosaic_zoom_reset();
                                } else {
                                    self.zoom = ZoomMode::Factor(1.0);
                                }
                            }
                            if self.view_mode == ViewMode::Mosaic {
                                mosaic_zoom_menu(ui, &r, self, menu_clicked);
                            } else {
                                zoom_menu(ui, &r, self, menu_clicked);
                            }
                            widgets::vline(ui, widgets::TOOL_H - 10.0);
                            // Tabs ⇄ Mosaic — a runtime-only arrangement,
                            // PixMe always opens in Tabs mode. In Mosaic the
                            // zoom group above drives the thumbnail scale.
                            let mosaic = self.view_mode == ViewMode::Mosaic;
                            let tip = if mosaic {
                                t!("viewer.mode.tabs.tooltip")
                            } else {
                                t!("viewer.mode.mosaic.tooltip")
                            };
                            // The toggle wears its destination: a mosaic
                            // icon while in tabs, a tab-strip icon while
                            // in mosaic — clicking switches the view.
                            let (icon, label) = if mosaic {
                                (Icon::PanelTop, t!("viewer.mode.tabs"))
                            } else {
                                (Icon::LayoutGrid, t!("viewer.mode.mosaic"))
                            };
                            if widgets::tool_button(ui, icon, &label, false, mosaic, true)
                                .on_hover_text(tip)
                                .clicked()
                            {
                                self.toggle_view_mode();
                            }
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
                                    unit: ResizeUnit::Pixels,
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
    /// frame, before `content()` measures, defaults to 100%). In Mosaic
    /// mode it is the thumbnail scale instead.
    fn zoom_label(&self) -> String {
        if self.view_mode == ViewMode::Mosaic {
            return format!("{}%", (self.mosaic_scale * 100.0).round() as u32);
        }
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
                menu_item(ui, shared, Command::OpenFile, t!("menu.file.open").as_ref());
                menu_item(
                    ui,
                    shared,
                    Command::ImportClipboard,
                    t!("menu.app.import_clipboard").as_ref(),
                );
                ui.separator();
                menu_item(
                    ui,
                    shared,
                    Command::SaveActive,
                    t!("menu.file.save").as_ref(),
                );
                menu_item(
                    ui,
                    shared,
                    Command::SaveAll,
                    t!("menu.file.save_all").as_ref(),
                );
                menu_item(
                    ui,
                    shared,
                    Command::RenameActive,
                    t!("menu.file.rename").as_ref(),
                );
                ui.separator();
                menu_item(
                    ui,
                    shared,
                    Command::PrintActive,
                    t!("menu.file.print").as_ref(),
                );
                ui.separator();
                menu_item(ui, shared, Command::Quit, t!("menu.app.quit").as_ref());
            });
            menu_button(ui, t!("menu.edit").as_ref(), |ui| {
                menu_item(
                    ui,
                    shared,
                    Command::UndoActive,
                    t!("menu.edit.undo").as_ref(),
                );
                menu_item(
                    ui,
                    shared,
                    Command::RedoActive,
                    t!("menu.edit.redo").as_ref(),
                );
                ui.separator();
                menu_item(
                    ui,
                    shared,
                    Command::CopyActive,
                    t!("menu.edit.copy").as_ref(),
                );
            });
            menu_button(ui, t!("menu.capture").as_ref(), |ui| {
                for (tip, mode) in [
                    ("toolbar.window.tooltip", CaptureMode::WindowOrObject),
                    ("toolbar.region.tooltip", CaptureMode::Region),
                    ("toolbar.fullscreen.tooltip", CaptureMode::FullScreen),
                    ("toolbar.fixed_region.tooltip", CaptureMode::FixedRegion),
                    ("toolbar.pin.tooltip", CaptureMode::Pin),
                ] {
                    menu_item(ui, shared, Command::Capture(mode), t!(tip).as_ref());
                }
            });
            menu_button(ui, t!("settings.title").as_ref(), |ui| {
                menu_item(
                    ui,
                    shared,
                    Command::OpenSettings,
                    t!("menu.app.preferences").as_ref(),
                );
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
        // A title shared with another tab can't save two files under
        // one name — it takes the danger tint as an inline warning.
        let dup = self.title_taken(&title, Some(index));
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
                        egui::Label::new(egui::RichText::new(title).color(if dup {
                            t.danger
                        } else if selected {
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
        ui.separator();
        // Tab ordering — persisted through settings but deliberately
        // menu-only (never shown in the settings window). Re-clicking
        // the active mode toggles back to insertion order.
        for (mode, key) in [
            (TabSort::ByName, "viewer.tab.sort_name"),
            (TabSort::ByDate, "viewer.tab.sort_date"),
        ] {
            let selected = self.sort_mode == Some(mode);
            if ui.selectable_label(selected, t!(key).to_string()).clicked() {
                self.sort_mode = if selected { None } else { Some(mode) };
                self.sort_tabs();
                let mut settings = shared.lock_settings();
                settings.tab_sort = self.sort_mode;
                settings.save();
                ui.close();
            }
        }
    }

    /// The document canvas: the active capture fit to the window (or at the
    /// explicit zoom, scrollable), plus the marquee-selection interaction.
    /// In Mosaic mode the whole content area becomes the thumbnail grid.
    fn content(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        if self.view_mode == ViewMode::Mosaic {
            self.mosaic_content(ui, shared);
            return;
        }
        if self.tabs.is_empty() {
            let t = theme::tokens(ui.ctx());
            // Restore in flight reads as "loading", not "empty" — the
            // tabs may arrive with it.
            let key = if self.loading_since.is_some() {
                "viewer.loading"
            } else {
                "viewer.empty"
            };
            ui.centered_and_justified(|ui| {
                ui.colored_label(t.text_dim, t!(key).to_string());
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
        let area = egui::ScrollArea::both().auto_shrink([false; 2]);
        let mut out = area.show(ui, |ui| {
            let canvas = size.max(avail.size());
            let (canvas_rect, resp) = ui.allocate_exact_size(canvas, Sense::click_and_drag());
            let img_rect = Rect::from_center_size(canvas_rect.center(), size);
            ui.painter().image(tex, img_rect, uv, egui::Color32::WHITE);
            // Double-click on the capture opens the Draw editor on
            // it — the same action as the toolbar's Pencil.
            if resp.double_clicked() {
                shared.push(Command::OpenDraw);
            }
            resp
        });
        // Hold-and-drag pans the zoomed canvas directly instead of the
        // scrollbars; a fitted image never overflows, so the gesture
        // stays inert until the view actually scrolls. The grab cursor
        // advertises the pan while the image overflows the window.
        let overflows = size.x > avail.width() || size.y > avail.height();
        if overflows && out.inner.hovered() && !out.inner.is_pointer_button_down_on() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if out.inner.dragged_by(egui::PointerButton::Primary) && overflows {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            let delta = ui.input(|i| i.pointer.delta());
            out.state.offset -= delta;
            out.state.store(ui.ctx(), out.id);
        }
    }

    /// The mosaic grid: one thumbnail tile per capture plus a trailing
    /// "new capture" tile, in a vertical scroll area whose column count
    /// adapts to the available width. Enter on the active tile opens it
    /// in Tabs mode (same as a double-click).
    fn mosaic_content(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let t = theme::tokens(ui.ctx());
        let avail = ui.available_rect_before_wrap();
        ui.painter().rect_filled(avail, 0.0, t.canvas);
        // Keyboard gate mirrors `show`'s: Enter must not fire while a
        // dialog or a text field is on top.
        let modal_open = self.confirm_close
            || self.resize.is_some()
            || self.rename.is_some()
            || self.protect.is_some()
            || self.confirm_delete.is_some()
            || shared.modal_child_open();
        // Base tile 220×160 pt at 100%, scaled by the mosaic zoom.
        let (tile_w, tile_h) = (220.0 * self.mosaic_scale, 160.0 * self.mosaic_scale);
        let spacing = 10.0;
        egui::ScrollArea::vertical()
            .id_salt("pixme-mosaic")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(spacing, spacing);
                // Grid margin — the selected tile's ring must not kiss
                // the window edge.
                ui.add_space(spacing);
                let cols = (ui.available_width() / (tile_w + spacing)).floor().max(1.0) as usize;
                let total = self.tabs.len() + 1; // one tile per tab + the "+" tile
                for row in (0..total).step_by(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = spacing;
                        ui.add_space(spacing);
                        for cell in row..(row + cols).min(total) {
                            if cell < self.tabs.len() {
                                self.mosaic_tile(ui, cell, shared, tile_w, tile_h);
                            } else {
                                self.mosaic_new_tile(ui, shared, tile_w, tile_h);
                            }
                        }
                    });
                }
                ui.add_space(spacing);
            });
        if !modal_open
            && !ui.ctx().egui_wants_keyboard_input()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && let Some(i) = self.active_index()
        {
            // The tile is already selected — no re-lock via set_active.
            self.open_active_tile(i);
        }
    }

    /// One mosaic tile: the capture's thumbnail (or a centred padlock
    /// for locked tabs), a dark caption band with the unsaved dot, the
    /// padlock and the elided title (plus the capture date on hover),
    /// and hover-only chrome — the close cross and the drag-out anchor.
    /// Single click selects, double-click/Enter opens the tile in Tabs
    /// mode, right-click opens the tab's context menu.
    fn mosaic_tile(&mut self, ui: &mut egui::Ui, index: usize, shared: &Shared, w: f32, h: f32) {
        // A previous tile's menu or cross may have closed tabs mid-
        // frame — the index is only valid while the tab still exists.
        if index >= self.tabs.len() {
            return;
        }
        let t = theme::tokens(ui.ctx());
        let selected = index == self.active;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        // The close cross and the drag anchor are registered every
        // frame (stable hover, like the tab chips) but painted only
        // while the pointer is over the tile.
        let close_rect = Rect::from_min_size(
            Pos2::new(rect.max.x - 26.0, rect.min.y + 6.0),
            Vec2::splat(20.0),
        );
        let anchor_rect =
            Rect::from_min_size(rect.min + Vec2::new(6.0, 6.0), Vec2::new(24.0, 20.0));
        let close = ui.interact(
            close_rect,
            ui.id().with(("pixme-tile-close", index)),
            Sense::click(),
        );
        let anchor = ui.interact(
            anchor_rect,
            ui.id().with(("pixme-tile-drag", index)),
            Sense::click(),
        );
        let tile_hovered = resp.hovered() || close.hovered() || anchor.hovered();
        // Frame: card at rest, accent-soft when selected — the tab
        // chips' selected treatment.
        ui.painter().rect(
            rect,
            CornerRadius::same(t.radius_md),
            if selected { t.accent_soft } else { t.card },
            Stroke::new(
                1.0,
                if selected {
                    t.accent_border
                } else {
                    t.card_border
                },
            ),
            StrokeKind::Inside,
        );
        // Pixels stay hidden for protected tabs: no thumbnail is ever
        // generated or painted, just a centred padlock — a proven
        // password unlocks the canvas, never the mosaic tile.
        let image_area = Rect::from_min_max(
            rect.min + Vec2::new(6.0, 6.0),
            Pos2::new(rect.max.x - 6.0, rect.max.y - BAND_H - 4.0),
        );
        if self.tabs[index].doc.meta.is_protected() {
            icons::paint(
                ui,
                Rect::from_center_size(image_area.center(), Vec2::splat(34.0)),
                Icon::Lock,
                34.0,
                t.text_dim,
            );
        } else if ui.is_rect_visible(rect) {
            self.ensure_thumb(ui, index);
            if let Some((_, thumb)) = &self.tabs[index].thumb {
                // Fit, never distorted: the thumbnail keeps its aspect.
                let img = fit_rect(image_area, thumb.size_vec2());
                ui.painter().image(
                    thumb.id(),
                    img,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
        self.mosaic_band(ui, index, rect, tile_hovered);
        // Hover chrome: the drag anchor (top-left) and the close cross
        // (top-right, red hover like the tabs and the settings window).
        if tile_hovered {
            anchor
                .clone()
                .on_hover_text(t!("viewer.mosaic.drag_tooltip"));
            if anchor.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            ui.painter().rect(
                anchor_rect,
                CornerRadius::same(t.radius_sm),
                theme::mosaic_chrome::ANCHOR_BG,
                Stroke::NONE,
                StrokeKind::Inside,
            );
            icons::paint(
                ui,
                anchor_rect,
                Icon::GripVertical,
                14.0,
                theme::mosaic_chrome::ANCHOR_ICON,
            );
            paint_tab_close(ui, close_rect, close.hovered());
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        // The cross closes only this capture — never a warning, like
        // the tabs. Guarded by `tile_hovered` so the invisible cross
        // zone stays inert while the pointer is elsewhere.
        if close.clicked() && tile_hovered {
            self.close_tabs(&[index]);
            return;
        }
        // Tile clicks (the cross and the anchor sit above and win).
        // Double-click opens the tile in Tabs mode — the selection
        // happened on the first click of the pair.
        if resp.double_clicked() {
            self.open_active_tile(index);
            return;
        }
        if resp.clicked() || resp.secondary_clicked() {
            // Selecting another tile re-locks every protected one.
            if index != self.active {
                self.set_active(index);
            }
        }
        egui::Popup::context_menu(&resp).show(|ui| {
            ui.set_min_width(200.0);
            self.tab_menu(ui, index, shared);
        });
    }

    /// The trailing "new capture" tile — the mosaic's twin of the tab
    /// strip's "+": clicking it brings the floating bar back.
    fn mosaic_new_tile(&mut self, ui: &mut egui::Ui, shared: &Shared, w: f32, h: f32) {
        let t = theme::tokens(ui.ctx());
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        let hovered = resp.hovered();
        ui.painter().rect(
            rect,
            CornerRadius::same(t.radius_md),
            if hovered { t.hover } else { t.card },
            Stroke::new(
                1.0,
                if hovered {
                    t.accent_border
                } else {
                    t.card_border
                },
            ),
            StrokeKind::Inside,
        );
        let plus = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.center().y - 10.0),
            Vec2::splat(24.0),
        );
        icons::paint(
            ui,
            plus,
            Icon::Plus,
            24.0,
            if hovered { t.accent } else { t.icon },
        );
        ui.painter().text(
            Pos2::new(rect.center().x, rect.max.y - 22.0),
            egui::Align2::CENTER_CENTER,
            t!("viewer.new_capture.tooltip"),
            theme::medium(10.5),
            t.text_dim,
        );
        let resp = resp.on_hover_text(t!("viewer.new_capture.tooltip"));
        if resp.clicked() {
            shared.push(Command::ShowBar);
        }
    }

    /// The tile's caption band: an always-visible dark strip with the
    /// unsaved dot, the padlock and the elided title; hovering adds the
    /// capture date in the active language's local format.
    fn mosaic_band(&self, ui: &mut egui::Ui, index: usize, rect: Rect, hovered: bool) {
        let t = theme::tokens(ui.ctx());
        let band = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - BAND_H), rect.max);
        ui.painter().rect(
            band,
            CornerRadius {
                nw: 0,
                ne: 0,
                sw: t.radius_md,
                se: t.radius_md,
            },
            theme::mosaic_chrome::BAND,
            Stroke::NONE,
            StrokeKind::Inside,
        );
        let tab = &self.tabs[index];
        let title = tab.title.clone();
        let (dirty, protected) = (!tab.saved, tab.doc.meta.is_protected());
        let date = hovered.then(|| {
            tab.doc
                .meta
                .captured_at
                .format(capture_date_fmt())
                .to_string()
        });
        let mut band_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(band.shrink2(Vec2::new(8.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        band_ui.spacing_mut().item_spacing.x = 4.0;
        if dirty {
            let (r, _) = band_ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
            band_ui.painter().circle_filled(r.center(), 3.0, t.danger);
        }
        if protected {
            let (r, _) = band_ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
            icons::image(Icon::Lock, 11.0, theme::mosaic_chrome::BAND_TEXT).paint_at(&band_ui, r);
        }
        band_ui.add(
            egui::Label::new(egui::RichText::new(title).color(theme::mosaic_chrome::BAND_TEXT))
                .truncate(),
        );
        if let Some(date) = date {
            band_ui.add(
                egui::Label::new(egui::RichText::new(date).color(theme::mosaic_chrome::BAND_DIM))
                    .truncate(),
            );
        }
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
        let t = theme::tokens(ctx);
        let resp = egui::Modal::new(egui::Id::new("pixme-resize"))
            .frame(
                egui::Frame::default()
                    .fill(t.dock)
                    .stroke(Stroke::new(1.0, t.card_border))
                    .corner_radius(CornerRadius::same(t.radius_lg)),
            )
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                // Title-bar band like the other modals: card tint rounded on
                // the top corners, with the painted close cross (red on
                // hover) instead of native chrome.
                egui::Frame::default()
                    .inner_margin(Margin::symmetric(12, 8))
                    .fill(t.card)
                    .corner_radius(CornerRadius {
                        nw: t.radius_lg,
                        ne: t.radius_lg,
                        sw: 0,
                        se: 0,
                    })
                    .show(ui, |ui| {
                        ui.set_width(340.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(t!("resize.title").to_string())
                                    .color(t.text)
                                    .strong(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let (rect, _) =
                                        ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                                    let resp = ui.interact(
                                        rect,
                                        ui.id().with("resize-close"),
                                        Sense::click(),
                                    );
                                    paint_tab_close(ui, rect, resp.hovered());
                                    if resp.clicked() {
                                        dismiss = true;
                                    }
                                },
                            );
                        });
                    });
                egui::Frame::default()
                    .inner_margin(Margin::symmetric(14, 14))
                    .show(ui, |ui| {
                        ui.set_width(320.0);
                        ui.spacing_mut().item_spacing.y = 7.0;
                        // Unit toggle: pixels or percentage of the original. Switching
                        // converts the draft so the target size never jumps.
                        let mut px_on = state.unit == ResizeUnit::Pixels;
                        let mut pct_on = state.unit == ResizeUnit::Percent;
                        ui.horizontal(|ui| {
                            ui.label(t!("resize.unit").to_string());
                            if ui
                                .toggle_value(&mut px_on, t!("resize.unit_px").to_string())
                                .clicked()
                            {
                                // Fields hold percentages — resolve them to pixels.
                                if let (Ok(w), Ok(h)) = (
                                    state.width.trim().parse::<f64>(),
                                    state.height.trim().parse::<f64>(),
                                ) {
                                    state.width = (orig_w as f64 * w / 100.0).round().to_string();
                                    state.height = (orig_h as f64 * h / 100.0).round().to_string();
                                }
                                state.unit = ResizeUnit::Pixels;
                            } else if ui
                                .toggle_value(&mut pct_on, t!("resize.unit_pct").to_string())
                                .clicked()
                            {
                                if let (Ok(w), Ok(h)) = (
                                    state.width.trim().parse::<f64>(),
                                    state.height.trim().parse::<f64>(),
                                ) {
                                    state.width =
                                        (w / orig_w.max(1) as f64 * 100.0).round().to_string();
                                    state.height =
                                        (h / orig_h.max(1) as f64 * 100.0).round().to_string();
                                }
                                state.unit = ResizeUnit::Percent;
                            }
                        });
                        // Original / new dimensions with MP count, like the
                        // functional reference's dialog — the new side follows
                        // the fields (percentages resolve against the original).
                        let dims = |w: f64, h: f64| format!("{w:.0}x{h:.0}  {:.2}MP", w * h / 1e6);
                        let new_dims = match state.unit {
                            ResizeUnit::Pixels => match (
                                state.width.trim().parse::<f64>(),
                                state.height.trim().parse::<f64>(),
                            ) {
                                (Ok(w), Ok(h)) if w > 0.0 && h > 0.0 => dims(w, h),
                                _ => "—".to_owned(),
                            },
                            ResizeUnit::Percent => match (
                                state.width.trim().parse::<f64>(),
                                state.height.trim().parse::<f64>(),
                            ) {
                                (Ok(wp), Ok(hp)) if wp > 0.0 && hp > 0.0 => {
                                    dims(orig_w as f64 * wp / 100.0, orig_h as f64 * hp / 100.0)
                                }
                                _ => "—".to_owned(),
                            },
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
                        // Editing one field rewrites the other while locked. In
                        // percent mode the same percentage applies to both sides,
                        // so the fields simply mirror each other.
                        if state.keep_ratio {
                            if state.unit == ResizeUnit::Percent {
                                if w_changed && let Ok(w) = state.width.trim().parse::<f64>() {
                                    state.height = w.round().to_string();
                                } else if h_changed
                                    && let Ok(h) = state.height.trim().parse::<f64>()
                                {
                                    state.width = h.round().to_string();
                                }
                            } else if w_changed && let Ok(w) = state.width.trim().parse::<f64>() {
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
                        // Right-aligned pair: Cancel rightmost, Apply to its left.
                        ui.horizontal(|ui| {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if widgets::secondary_button(ui, &t!("save.cancel")).clicked() {
                                    dismiss = true;
                                }
                                if widgets::primary_button(ui, &t!("resize.apply")).clicked() {
                                    apply = true;
                                }
                            });
                        });
                    });
            });
        // Backdrop click or Escape = Cancel.
        if resp.should_close() {
            dismiss = true;
        }
        let mut applied = false;
        if apply {
            // Resolve the draft into target pixels (percent mode scales
            // the original); invalid input only flags the dialog — it
            // never reaches the resampler.
            let resolved: Option<(u32, u32)> = match state.unit {
                ResizeUnit::Pixels => match (
                    state.width.trim().parse::<f64>(),
                    state.height.trim().parse::<f64>(),
                ) {
                    (Ok(w), Ok(h)) if w > 0.0 && h > 0.0 => {
                        Some((w.round() as u32, h.round() as u32))
                    }
                    _ => None,
                },
                ResizeUnit::Percent => match (
                    state.width.trim().parse::<f64>(),
                    state.height.trim().parse::<f64>(),
                ) {
                    (Ok(wp), Ok(hp)) if wp > 0.0 && hp > 0.0 => Some((
                        (orig_w as f64 * wp / 100.0).round().max(1.0) as u32,
                        (orig_h as f64 * hp / 100.0).round().max(1.0) as u32,
                    )),
                    _ => None,
                },
            };
            match resolved {
                Some((w, h)) => {
                    self.resize_active(w, h, state.anti_alias);
                    applied = true;
                }
                None => state.invalid = true,
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
        // Tabs after the active one in the current order — the follow
        // count can never exceed them.
        let max_follow = self
            .active_index()
            .map(|i| self.tabs.len().saturating_sub(i + 1))
            .unwrap_or(0);
        let resp = egui::Modal::new(egui::Id::new("pixme-rename")).show(ctx, |ui| {
            ui.set_width(300.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(t!("dialog.rename.title").to_string())
                        .font(theme::semibold(14.0)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if dialog_close(ui) {
                        cancel = true;
                    }
                });
            });
            widgets::hairline(ui);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(t!("dialog.rename.label").to_string());
                let field = ui.add(
                    egui::TextEdit::singleline(&mut state.draft)
                        .desired_width(200.0)
                        .font(theme::medium(12.0)),
                );
                // Focus once on open — every frame would steal it back
                // from the follow field.
                if !state.focused {
                    field.request_focus();
                    state.focused = true;
                }
                // Enter applies, like the Rename button.
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    apply = true;
                }
            });
            // Optional batch rename: the current tab plus the next
            // `follow` tabs become `{name} - 1`, ` - 2`… in the current
            // order. Digits-only field, clamped on use; disabled when
            // the active tab is already last.
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(t!("dialog.rename.follow").to_string());
                ui.add_enabled(
                    max_follow > 0,
                    egui::DragValue::new(&mut state.follow)
                        .range(0..=max_follow)
                        .speed(1.0),
                );
            });
            let follow = state.follow.min(max_follow);
            if follow > 0 {
                let base = state.draft.trim();
                let mut names: Vec<String> = (1..=(follow + 1).min(4))
                    .map(|k| format!("{base} - {k}"))
                    .collect();
                if follow + 1 > 4 {
                    names.push("…".to_owned());
                }
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{} {}",
                        t!("dialog.rename.preview"),
                        names.join(", ")
                    ))
                    .color(t.text_dim)
                    .font(theme::medium(10.5)),
                );
            }
            if state.taken {
                ui.add_space(4.0);
                ui.colored_label(t.danger, t!("dialog.rename.empty").to_string());
            }
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::secondary_button(ui, &t!("save.cancel")).clicked() {
                    cancel = true;
                }
                if widgets::primary_button(ui, &t!("dialog.rename.ok")).clicked() {
                    apply = true;
                }
            });
        });
        if resp.should_close() {
            cancel = true;
        }
        if apply {
            let name = state.draft.trim();
            // Empty only: duplicate titles are allowed — save-all is
            // where a clash is reported and fixed by renaming.
            let taken = name.is_empty();
            if taken {
                state.taken = true;
            } else {
                self.apply_rename(name.to_owned(), state.follow.min(max_follow));
            }
            if !taken {
                return;
            }
        }
        if !(cancel || (apply && !state.taken)) {
            self.rename = Some(state);
        }
    }

    /// Save-all blocked by duplicate titles: two unsaved captures share
    /// a name; the only way forward is renaming, so one OK dismisses it.
    fn dup_names_dialog(&mut self, ctx: &egui::Context) {
        if !self.dup_names {
            return;
        }
        egui::Window::new(t!("dialog.dup_names.title").to_string())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(t!("dialog.dup_names.body").to_string());
                ui.add_space(8.0);
                if ui.button(t!("draw.ok").to_string()).clicked() {
                    self.dup_names = false;
                }
            });
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
                // Arming (or changing) the password locks the tab
                // immediately — same instant-padlock rule as the mosaic
                // tiles. Only DISARMING (an empty new password) leaves
                // it unlocked. The thumbnail is always dropped: locked
                // tabs never hold pixels.
                tab.unlocked = state.new.is_empty();
                tab.thumb = None;
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
/// Numeric-aware title compare so `Capture 9` sorts before `Capture 10`.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::Equal;
    let (mut a, mut b) = (a, b);
    while let (Some(ca), Some(cb)) = (a.chars().next(), b.chars().next()) {
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let (na, ta) = take_digits(a);
            let (nb, tb) = take_digits(b);
            match na
                .parse::<u64>()
                .unwrap_or(0)
                .cmp(&nb.parse::<u64>().unwrap_or(0))
            {
                Equal => {
                    a = ta;
                    b = tb;
                }
                o => return o,
            }
        } else if ca == cb {
            a = &a[ca.len_utf8()..];
            b = &b[cb.len_utf8()..];
        } else {
            return ca.cmp(&cb);
        }
    }
    a.len().cmp(&b.len())
}

/// Splits the leading ASCII-digit run off `s`.
fn take_digits(s: &str) -> (&str, &str) {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s.split_at(end)
}

/// The capture-date format for the active language — shared by the
/// status strip and the mosaic tiles' hovered bands.
fn capture_date_fmt() -> &'static str {
    match &*rust_i18n::locale() {
        "fr" | "es" => "%d/%m/%Y %H:%M",
        _ => "%b %d, %Y %H:%M",
    }
}

/// The largest centred rect with `inner`'s aspect ratio that fits in
/// `outer` — the mosaic thumbnail's "fit, never distorted" box.
fn fit_rect(outer: Rect, inner: Vec2) -> Rect {
    let scale = (outer.width() / inner.x.max(1.0)).min(outer.height() / inner.y.max(1.0));
    Rect::from_center_size(outer.center(), inner * scale)
}

/// The session-restore spinner: a faint track ring plus a rotating
/// accent arc, centred on the content area.
fn paint_loading(ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let center = ui.max_rect().center();
    let r = 22.0;
    ui.painter()
        .circle_stroke(center, r, Stroke::new(2.5, t.field_border));
    let head = ui.ctx().input(|i| i.time) * 3.0;
    let sweep = std::f64::consts::TAU * 0.72;
    let pts: Vec<egui::Pos2> = (0..=48)
        .map(|i| {
            let a = (head + i as f64 / 48.0 * sweep) as f32;
            center + Vec2::new(a.cos(), a.sin()) * r
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(pts, Stroke::new(2.5, t.accent)));
}

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

/// A dialog's top-right close cross — the tab-close style (red soft
/// fill + red X on hover). Returns `clicked`.
fn dialog_close(ui: &mut egui::Ui) -> bool {
    let t = theme::tokens(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        if hovered {
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
    resp.clicked()
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

/// The mosaic zoom menu: the fixed 50→200% tile-scale presets. The ±
/// buttons use ×1.25/÷1.25 arithmetic, so an intermediate scale shows
/// no preset selected.
fn mosaic_zoom_menu(ui: &egui::Ui, resp: &Response, v: &mut Viewer, open: bool) {
    tool_popup(ui, resp, "mosaic-zoom-menu", open, |ui| {
        for pct in [50u32, 75, 100, 125, 150, 200] {
            let scale = pct as f32 / 100.0;
            if ui
                .selectable_label((v.mosaic_scale - scale).abs() < 0.001, format!("{pct}%"))
                .clicked()
            {
                v.mosaic_scale = scale;
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

/// One menu entry dispatching a [`Command`] when clicked, greyed out
/// when the command is forbidden by the current modal state.
fn menu_item(ui: &mut egui::Ui, shared: &Shared, command: Command, label: &str) {
    let enabled = shared.command_enabled(&command);
    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
        shared.push(command);
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
            thumb: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            anno_undo: VecDeque::new(),
            anno_redo: VecDeque::new(),
            unlocked: false,
            insertion: v.next_insertion,
            persisted_rev: None,
            base_dirty: true,
        });
        v.next_insertion += 1;
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
    fn renaming_a_saved_tab_marks_it_unsaved() {
        let mut v = Viewer::default();
        v.push_stub("a");
        v.mark_saved(0, std::path::PathBuf::from("/tmp/a.png"));
        v.recompute_dirty();
        assert!(!v.dirty);

        v.set_active_title("b".to_owned());
        assert!(!v.tabs[0].saved, "renamed saved tab is unsaved again");
        assert!(v.dirty);
        assert_eq!(
            v.tabs[0].doc.meta.source_path.as_deref(),
            Some(std::path::Path::new("/tmp/a.png")),
            "source kept: Enregistrer still proposes that folder"
        );

        // Re-applying the SAME name changes nothing — it stays saved.
        v.mark_saved(0, std::path::PathBuf::from("/tmp/b.png"));
        v.recompute_dirty();
        v.set_active_title("b".to_owned());
        assert!(v.tabs[0].saved);
        assert!(!v.dirty);
    }

    #[test]
    fn mark_saved_renames_the_tab_to_the_written_stem() {
        let mut v = Viewer::default();
        v.push_stub("capture 1");
        v.mark_saved(0, std::path::PathBuf::from("/tmp/my new name.png"));
        assert_eq!(v.tabs[0].title, "my new name");
        assert!(v.tabs[0].saved);
        assert_eq!(
            v.tabs[0].doc.meta.source_path.as_deref(),
            Some(std::path::Path::new("/tmp/my new name.png"))
        );
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

    #[test]
    fn mosaic_mode_defaults_to_tabs_and_toggles() {
        let mut v = Viewer::default();
        assert_eq!(v.view_mode, ViewMode::Tabs, "PixMe always opens in tabs");
        assert_eq!(v.mosaic_scale, 1.0);
        v.toggle_view_mode();
        assert_eq!(v.view_mode, ViewMode::Mosaic);
        v.toggle_view_mode();
        assert_eq!(v.view_mode, ViewMode::Tabs);
    }

    #[test]
    fn mosaic_zoom_steps_and_clamps() {
        let mut v = Viewer::default();
        v.mosaic_zoom_in();
        assert!((v.mosaic_scale - 1.25).abs() < 1e-4);
        v.mosaic_zoom_in();
        assert!((v.mosaic_scale - 1.5625).abs() < 1e-4);
        // 200% is the ceiling: further steps never exceed it.
        for _ in 0..4 {
            v.mosaic_zoom_in();
        }
        assert_eq!(v.mosaic_scale, 2.0);
        // 50% is the floor.
        for _ in 0..20 {
            v.mosaic_zoom_out();
        }
        assert_eq!(v.mosaic_scale, 0.5);
        v.mosaic_zoom_reset();
        assert_eq!(v.mosaic_scale, 1.0);
        // The zoom caption follows the scale in Mosaic mode.
        v.toggle_view_mode();
        v.mosaic_zoom_in();
        assert_eq!(v.zoom_label(), "125%");
    }

    #[test]
    fn mosaic_thumbnail_scales_the_longest_side() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut v = Viewer::default();
            push_doc(&mut v, 2000, 1000);
            v.ensure_thumb(ui, 0);
            let (_, tex) = v.tabs[0].thumb.as_ref().expect("thumbnail missing");
            assert_eq!(
                tex.size(),
                [480, 240],
                "aspect ratio kept, longest side capped"
            );
        });
        out.textures_delta.clear();
    }

    #[test]
    fn locked_tabs_never_hold_a_thumbnail() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut v = Viewer::default();
            v.push_stub("secret");
            v.tabs[0].doc.meta.set_password("pw");
            assert!(v.is_locked(0));
            v.ensure_thumb(ui, 0);
            assert!(
                v.tabs[0].thumb.is_none(),
                "a locked tab must not generate pixel data"
            );
            // A proven password unlocks the canvas, but the mosaic tile
            // stays a padlock — protected tabs never build thumbnails.
            v.tabs[0].unlocked = true;
            assert!(!v.is_locked(0));
            v.ensure_thumb(ui, 0);
            assert!(
                v.tabs[0].thumb.is_none(),
                "a protected tab never shows its pixels in the mosaic"
            );
            // …and leaving the tab re-locks it.
            v.set_active(0);
            assert!(v.is_locked(0));
        });
        out.textures_delta.clear();
    }

    #[test]
    fn mosaic_selects_and_closes_like_the_tab_strip() {
        let mut v = Viewer::default();
        v.toggle_view_mode();
        for t in ["a", "b", "c"] {
            v.push_stub(t);
        }
        // Single click selects without leaving the mosaic.
        v.set_active(1);
        assert_eq!(v.view_mode, ViewMode::Mosaic);
        // The tile's close cross closes only that capture.
        v.close_tabs(&[1]);
        assert_eq!(v.tabs.len(), 2);
        assert_eq!(v.active, 1, "active shifts onto the next capture");
        // Double-click (or Enter) opens the active tile in tabs mode.
        v.open_active_tile(v.active);
        assert_eq!(v.view_mode, ViewMode::Tabs);
        assert_eq!(v.tabs[v.active].title, "c");
    }

    #[test]
    fn opening_the_active_tile_does_not_relock() {
        let mut v = Viewer::default();
        v.push_stub("a");
        v.push_stub("b");
        v.tabs[0].doc.meta.set_password("pw");
        v.set_active(0);
        v.tabs[0].unlocked = true; // the password was proved
        assert!(!v.is_locked(0));
        v.open_active_tile(0);
        assert_eq!(v.view_mode, ViewMode::Tabs);
        assert!(
            !v.is_locked(0),
            "opening the already-active tile is not a selection change"
        );
        // Selecting a DIFFERENT tile re-locks it, like the tab strip.
        v.tabs[1].doc.meta.set_password("pw2");
        v.view_mode = ViewMode::Mosaic;
        v.open_active_tile(1);
        assert_eq!(v.view_mode, ViewMode::Tabs);
        assert!(v.is_locked(0), "switching selection re-locks the old tab");
        assert!(v.is_locked(1), "a protected tile opens locked");
    }

    #[test]
    fn commit_draw_noop_keeps_saved_and_real_edit_clears_pixel_undo() {
        let mut v = Viewer::default();
        push_doc(&mut v, 20, 20);
        v.resize_active(10, 10, false);
        v.tabs[0].saved = true;
        v.recompute_dirty();
        assert_eq!(v.tabs[0].undo.len(), 1);

        // No-op Draw commit: saved stays true, pixel undo stays intact.
        let (tab, _, doc, undo, redo) = v.draw_parts().unwrap();
        v.commit_draw(tab, doc, undo, redo);
        assert!(v.tabs[0].saved);
        assert!(!v.dirty);
        assert_eq!(v.tabs[0].undo.len(), 1);

        // Real annotation edit in Draw: marks unsaved and clears pixel undo
        // immediately (without waiting for mark_saved).
        let (tab, title, doc, undo, redo) = v.draw_parts().unwrap();
        let mut session = crate::ui::draw::DrawSession::open(
            doc,
            tab,
            title,
            undo,
            redo,
            &crate::config::Settings::default(),
        );
        let id = session.doc.alloc_id();
        let anno = crate::editor::document::Annotation {
            id,
            kind: crate::editor::document::AnnotationKind::Step(crate::editor::document::StepObj {
                pos: egui::pos2(2.0, 2.0),
                count: 1,
                mode: crate::editor::document::StepMode::Numbers,
                style: crate::editor::document::StepStyle::Circle,
                size: crate::editor::document::StepSize::Medium,
                bg: Color32::RED,
                fg: Color32::WHITE,
                ring: 0.0,
                ring_color: Color32::WHITE,
                opacity: 1.0,
                shadow: 0.0,
                lower: false,
            }),
        };
        session.doc.annotations.push(anno.clone());
        session.undo.push_back(AnnoEdit::Add { anno, index: 0 });
        session.doc.revision += 1;
        assert!(session.has_uncommitted_edits());

        // Undoing back to the open state lets request_cancel exit silently.
        session.undo_once();
        assert!(!session.has_uncommitted_edits());
        session.request_cancel();
        assert!(session.outcome() == Some(crate::ui::draw::Outcome::Cancel));

        // Re-apply and commit to the viewer.
        session.redo_once();
        v.commit_draw(session.tab, session.doc, session.undo, session.redo);
        assert!(!v.tabs[0].saved);
        assert!(v.dirty);
        assert!(
            v.tabs[0].undo.is_empty(),
            "pixel op history cleared on annotation commit"
        );
    }

    #[test]
    fn restore_with_preexisting_live_tab_keeps_persisted_sigs_unset() {
        let mut store = crate::store::Store::open_in_memory();
        let mut old = Viewer::default();
        old.push_stub("from_db");
        old.tabs[0].saved = true;
        old.recompute_dirty();
        store.save_session(&old.tabs, 0, old.next_title());

        // A new session where a tab arrived before restore_session landed.
        let mut v = Viewer::default();
        v.push_stub("live_capture");
        v.restore_session(store.load_session(), None);
        assert_eq!(v.tabs.len(), 2);
        assert_eq!(v.tabs[0].title, "from_db");
        assert_eq!(v.tabs[1].title, "live_capture");
        assert_eq!(v.active, 1, "live tab stays active");
        assert!(
            v.persisted_sigs.is_none(),
            "unpersisted live tab must force a save on quit"
        );
    }
}
