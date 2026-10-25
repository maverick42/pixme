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
    Align, CentralPanel, CornerRadius, Layout, Margin, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, Vec2, ViewportCommand,
};
use rust_i18n::t;

use crate::app::{CaptureMode, Command, Shared};
use crate::editor::Document;
use crate::ui::icons::Icon;
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
    /// A clipboard image dropped onto this document (Coller) — floats
    /// until the user clicks outside it, which flattens it in place.
    pasted: Option<PastedLayer>,
    /// Reversible pixel mutations, oldest → newest (session-only).
    undo: VecDeque<PixelOp>,
    /// Ops undone, reappliable until the next mutation clears them.
    redo: VecDeque<PixelOp>,
}

/// How many pixel mutations stay undoable per tab. Full-image ops keep
/// one pre-image each (~20 MB on a Retina capture), so the cap bounds
/// the worst case; paste steps only store the covered patch.
const HISTORY_CAP: usize = 10;

/// One reversible pixel-level mutation of a tab's base image — stored
/// per op rather than as full snapshots wherever a small patch suffices.
enum PixelOp {
    /// `pasted` was alpha-overlaid at `pos`; `under` is what it covered
    /// (the visible part only, so both stay small).
    Flatten {
        pos: (u32, u32),
        pasted: image::RgbaImage,
        under: image::RgbaImage,
    },
    /// Cropped to `rect`; `before` is the whole pre-crop image.
    Crop {
        before: image::RgbaImage,
        rect: (u32, u32, u32, u32),
    },
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
            Self::Flatten { pos, under, .. } => image::imageops::replace(
                &mut doc.base.image,
                under,
                i64::from(pos.0),
                i64::from(pos.1),
            ),
            Self::Crop { before, .. } | Self::Resize { before, .. } => {
                doc.base.image = before.clone();
            }
        }
    }

    /// Re-applies the mutation on `doc`'s base image after an undo.
    fn redo(&self, doc: &mut Document) {
        match self {
            Self::Flatten { pos, pasted, .. } => image::imageops::overlay(
                &mut doc.base.image,
                pasted,
                i64::from(pos.0),
                i64::from(pos.1),
            ),
            Self::Crop { rect, .. } => {
                doc.base.image =
                    image::imageops::crop_imm(&doc.base.image, rect.0, rect.1, rect.2, rect.3)
                        .to_image();
            }
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

/// What `content()` hands to `paste_layer` — everything it needs to
/// paint and place the floating paste without re-borrowing the tab.
struct PasteView {
    /// The layer's texture.
    tex: egui::TextureId,
    /// Bitmap size in image physical px.
    size: Vec2,
    /// The copied marquee shape, for the outline.
    shape: SelectShape,
}

/// The floating pasted bitmap: draggable, flattened on click-outside
/// or Enter, discarded on Escape — the functional reference's paste.
struct PastedLayer {
    /// The pixels to composite (corners transparent for shaped copies).
    image: image::RgbaImage,
    /// Top-left corner in image physical px (drag updates it).
    pos: Pos2,
    /// The marquee shape the bitmap was copied with — drives the
    /// outline only (the pixels already carry the shape).
    shape: SelectShape,
    /// Display texture, loaded lazily once.
    tex: Option<egui::TextureHandle>,
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

/// The marquee outline the Rectangle tool draws — crop is always the
/// bounding rect whatever the shape.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum SelectShape {
    /// Plain rectangle (dashed outline).
    #[default]
    Rect,
    /// Ellipse inside the drag bounds.
    Oval,
    /// Rounded rectangle.
    Round,
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
    /// The Rectangle tool: armed, a marquee drag on the canvas selects.
    select_tool: bool,
    /// Marquee outline shape chosen in the tool's menu.
    select_shape: SelectShape,
    /// The live marquee selection, in image physical px.
    selection: Option<Rect>,
    /// The shape the live selection was drawn with — Copier masks by
    /// this, not by whatever shape is armed now.
    selection_shape: Option<SelectShape>,
    /// The shape of the last in-app copy — remembered (not consumed) so
    /// repeated pastes of the same clipboard keep their outline.
    copy_shape: Option<SelectShape>,
    /// Marquee drag anchor (image px) while drawing.
    marquee_anchor: Option<Pos2>,
    /// The resize dialog, open while Some.
    resize: Option<ResizeDialog>,
    /// The rename prompt's draft, open while Some.
    rename: Option<String>,
}

impl Tab {
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
    pub fn push_named_document(&mut self, doc: Document, title: String, saved: bool) {
        self.tabs.push(Tab {
            title,
            doc,
            saved,
            texture: None,
            pasted: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
        });
        self.active = self.tabs.len() - 1;
        // A new document gets a fresh view — a stale marquee/zoom would
        // leak onto unrelated pixels.
        self.reset_view();
        self.recompute_dirty();
    }

    /// Test helper: a tab whose document wraps a 1×1 capture.
    #[cfg(test)]
    pub fn push_stub(&mut self, title: &str) {
        use crate::capture::{Capture, MonitorInfo, PhysicalRect};
        self.tabs.push(Tab {
            title: title.to_string(),
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
            pasted: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
        });
    }

    /// The session's dirty flag = any tab not written to disk.
    pub fn recompute_dirty(&mut self) {
        self.dirty = self.tabs.iter().any(|t| !t.saved);
    }

    /// Marks `index` saved and refreshes the session flag — called after a
    /// save-as dialog completes for a viewer document.
    pub fn mark_saved(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.saved = true;
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

    /// `(index, title, base capture)` of the active tab — everything the
    /// save-as dialog needs to own (the capture is cloned).
    pub fn save_target(&self) -> Option<(usize, String, crate::capture::Capture)> {
        let i = self.active_index()?;
        let tab = &self.tabs[i];
        Some((i, tab.title.clone(), tab.doc.base.clone()))
    }

    /// Opens the rename prompt on the active tab (File → Rename).
    pub fn open_rename(&mut self) {
        if let Some(title) = self.active_title() {
            self.rename = Some(title);
        }
    }

    /// What Copier puts on the clipboard: the marquee selection when one
    /// is live, the whole document otherwise.
    pub fn copy_image(&mut self) -> Option<image::RgbaImage> {
        let i = self.active_index()?;
        let img = &self.tabs[i].doc.base.image;
        let mut out = match self.selection {
            Some(sel) => {
                let sel = clamp_to_image(sel, img.width(), img.height());
                if sel.width() < 1.0 || sel.height() < 1.0 {
                    img.clone()
                } else {
                    image::imageops::crop_imm(
                        img,
                        sel.min.x as u32,
                        sel.min.y as u32,
                        sel.width() as u32,
                        sel.height() as u32,
                    )
                    .to_image()
                }
            }
            None => img.clone(),
        };
        // A shaped marquee copies shaped pixels: outside the ellipse /
        // rounded corners goes transparent, which is what the paste then
        // composites — the functional reference behaves the same way.
        if self.selection.is_some() {
            match self.selection_shape.unwrap_or_default() {
                SelectShape::Rect => {}
                SelectShape::Oval => mask_ellipse(&mut out),
                SelectShape::Round => mask_rounded(
                    &mut out,
                    // The marquee's 8pt corners, in image physical px.
                    8.0 * f64::from(self.tabs[i].doc.base.scale_factor.max(0.25)),
                ),
            }
        }
        // The shape survives the paste: repeated Coller of the same
        // clipboard still outlines like the copy that produced it.
        self.copy_shape = self.selection_shape;
        Some(out)
    }

    /// The whole document image, for print (selections don't apply).
    pub fn active_image(&self) -> Option<image::RgbaImage> {
        let i = self.active_index()?;
        Some(self.tabs[i].doc.base.image.clone())
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

    /// Drops `image` onto the active document as a floating pasted layer
    /// (the Coller action — paste never creates a tab, only
    /// File→Import does). `false` when there is no document.
    pub fn paste_active(&mut self, image: image::RgbaImage) -> bool {
        let Some(i) = self.active_index() else {
            return false;
        };
        // A still-floating layer flattens before the new one drops.
        self.commit_paste(i);
        // The live marquee yields to the layer. The outline comes from
        // the shape the copy recorded — solid-cornered bitmaps (external
        // content) always read as plain rects.
        let shape = paste_outline_shape(&image, self.copy_shape);
        self.selection = None;
        self.selection_shape = None;
        self.tabs[i].pasted = Some(PastedLayer {
            image,
            pos: Pos2::ZERO,
            shape,
            tex: None,
        });
        true
    }

    /// Flattens the floating paste of tab `index` into its document —
    /// alpha-composited at its position, clipped to the image bounds.
    fn commit_paste(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(pasted) = tab.pasted.take() else {
            return;
        };
        let (x, y) = (pasted.pos.x as u32, pasted.pos.y as u32);
        // The layer can overflow the image when it's larger than the
        // document — the undo patch only covers the visible part.
        let vis_w = pasted
            .image
            .width()
            .min(tab.doc.base.image.width().saturating_sub(x));
        let vis_h = pasted
            .image
            .height()
            .min(tab.doc.base.image.height().saturating_sub(y));
        let under = image::imageops::crop_imm(&tab.doc.base.image, x, y, vis_w, vis_h).to_image();
        image::imageops::overlay(
            &mut tab.doc.base.image,
            &pasted.image,
            i64::from(x),
            i64::from(y),
        );
        tab.push_op(PixelOp::Flatten {
            pos: (x, y),
            pasted: pasted.image,
            under,
        });
        tab.doc.annotations.clear();
        tab.doc.revision += 1;
        tab.saved = false;
        self.dirty = true;
    }

    /// Whether the active tab carries a floating paste — the marquee
    /// steps aside while one floats.
    fn active_has_paste(&self) -> bool {
        self.active_index()
            .is_some_and(|i| self.tabs[i].pasted.is_some())
    }

    /// ⌘Z — reverts the newest pixel mutation of the active tab. A
    /// floating paste counts as the freshest edit: undo abandons it.
    pub fn undo_active(&mut self) -> bool {
        let Some(i) = self.active_index() else {
            return false;
        };
        if self.tabs[i].pasted.take().is_some() {
            return true;
        }
        let Some(op) = self.tabs[i].undo.pop_back() else {
            return false;
        };
        let tab = &mut self.tabs[i];
        op.undo(&mut tab.doc);
        tab.doc.revision += 1;
        tab.saved = false;
        tab.redo.push_back(op);
        self.dirty = true;
        self.selection = None;
        self.selection_shape = None;
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
        self.selection = None;
        self.selection_shape = None;
        true
    }

    /// Crops the active document to `sel` (image physical px) — the
    /// Rectangle tool's double-click/Enter action. Annotations are dropped:
    /// their physical-px coordinates would need a translation pass the
    /// editor model doesn't have yet.
    fn crop_active(&mut self, sel: Rect) {
        let Some(i) = self.active_index() else { return };
        // A floating paste flattens before the image changes under it.
        self.commit_paste(i);
        let tab = &mut self.tabs[i];
        let sel = clamp_to_image(sel, tab.doc.base.image.width(), tab.doc.base.image.height());
        if sel.width() < 1.0 || sel.height() < 1.0 {
            return;
        }
        let before = tab.doc.base.image.clone();
        let rect = (
            sel.min.x as u32,
            sel.min.y as u32,
            sel.width() as u32,
            sel.height() as u32,
        );
        tab.doc.base.image =
            image::imageops::crop_imm(&tab.doc.base.image, rect.0, rect.1, rect.2, rect.3)
                .to_image();
        tab.push_op(PixelOp::Crop { before, rect });
        tab.doc.annotations.clear();
        tab.doc.revision += 1;
        tab.saved = false;
        self.dirty = true;
        self.selection = None;
        self.selection_shape = None;
    }

    /// Resamples the active document to `w × h` physical px (the resize
    /// dialog's Apply); `antialias` picks Lanczos3 over nearest-neighbour.
    /// Same annotation caveat as [`Self::crop_active`].
    fn resize_active(&mut self, w: u32, h: u32, antialias: bool) {
        let Some(i) = self.active_index() else { return };
        if w == 0 || h == 0 {
            return;
        }
        // A floating paste flattens before the image changes under it.
        self.commit_paste(i);
        let tab = &mut self.tabs[i];
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
        tab.doc.annotations.clear();
        tab.doc.revision += 1;
        tab.saved = false;
        self.dirty = true;
        self.selection = None;
        self.selection_shape = None;
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
        self.select_tool = false;
        self.selection = None;
        self.selection_shape = None;
        self.marquee_anchor = None;
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
            || shared.modal_child_open();
        // Canvas keyboard: Enter commits the floating paste or crops the
        // marquee, Esc discards the paste or clears the selection. The
        // OS copy/paste conventions (⌘/Ctrl+C/V) work everywhere in the
        // window — text edits keep their own handling.
        if !modal_open {
            if !ctx.egui_wants_keyboard_input() {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)) {
                    shared.push(Command::CopyActive);
                    // The push lands mid-frame: wake the next pass so
                    // pump() dispatches it even without mouse activity.
                    ctx.request_repaint();
                }
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::V)) {
                    shared.push(Command::PasteActive);
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
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Some(i) = self
                    .active_index()
                    .filter(|i| self.tabs[*i].pasted.is_some())
                {
                    self.commit_paste(i);
                } else if let Some(sel) = self.selection {
                    self.crop_active(sel);
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                if let Some(i) = self
                    .active_index()
                    .filter(|i| self.tabs[*i].pasted.is_some())
                {
                    self.tabs[i].pasted = None;
                } else {
                    self.selection = None;
                    self.selection_shape = None;
                    self.select_tool = false;
                    self.marquee_anchor = None;
                }
            }
        }
        // Explicit fill: the app-wide clear color is transparent (the anchor
        // and the floating bar need it), so windows must paint their own.
        let t = theme::tokens(&ctx);
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
                // The band scrolls sideways when the window gets too
                // narrow for every button.
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
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
                            // Rectangle ▾ — arms the marquee, caret picks the shape.
                            let r = widgets::tool_button(
                                ui,
                                Icon::SquareDashed,
                                &t!("vbar.select.label"),
                                true,
                                self.select_tool,
                                has_doc,
                            );
                            let menu_clicked = r.clicked() && widgets::caret_hit(&r, true);
                            if r.clicked() && !menu_clicked {
                                self.select_tool = !self.select_tool;
                                if !self.select_tool {
                                    self.selection = None;
                                    self.selection_shape = None;
                                }
                            }
                            shape_menu(ui, &r, self, menu_clicked);
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
                            // Draw mode lands in the next phase: visible, disabled.
                            widgets::tool_button(
                                ui,
                                Icon::Pencil,
                                &t!("vbar.draw.label"),
                                false,
                                false,
                                false,
                            );
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
                            if widgets::tool_button(
                                ui,
                                Icon::ClipboardPaste,
                                &t!("vbar.paste.label"),
                                false,
                                false,
                                true,
                            )
                            .clicked()
                            {
                                // Coller = floating layer on the active
                                // document (ImportClipboard opens a tab).
                                shared.push(Command::PasteActive);
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
                menu_item(ui, t!("menu.edit.paste").as_ref(), || {
                    shared.push(Command::PasteActive)
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

    /// The capture tab strip: one selectable label per capture, then "+".
    fn tab_strip(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        egui::Frame::default()
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
                    for index in 0..self.tabs.len() {
                        if ui
                            .selectable_label(index == self.active, &self.tabs[index].title)
                            .clicked()
                            && index != self.active
                        {
                            self.active = index;
                            self.reset_view();
                        }
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
        let tab = &mut self.tabs[index];
        let (img_w, img_h) = (
            tab.doc.base.image.width() as usize,
            tab.doc.base.image.height() as usize,
        );
        // The capture is in physical pixels; the viewer shows it at its
        // logical size (image px ÷ scale factor). Fit mode never upscales.
        let sf = tab.doc.base.scale_factor.max(0.25);
        if tab.texture.as_ref().map(|(rev, _)| *rev) != Some(tab.doc.revision) {
            let handle = ui.ctx().load_texture(
                format!("doc-{index}-{}", tab.doc.revision),
                egui::ColorImage::from_rgba_unmultiplied(
                    [img_w, img_h],
                    tab.doc.base.image.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            );
            tab.texture = Some((tab.doc.revision, handle));
        }
        // Copy the ids out: `tab`'s borrow ends here so the scroll
        // closure can take `&mut self` (marquee / paste interaction).
        let tex = tab.texture.as_ref().map(|(_, h)| h.id());
        // The floating paste gets its own texture, loaded once per drop.
        let pasted = tab.pasted.as_mut().map(|p| {
            if p.tex.is_none() {
                p.tex = Some(ui.ctx().load_texture(
                    format!("paste-{index}"),
                    egui::ColorImage::from_rgba_unmultiplied(
                        [p.image.width() as usize, p.image.height() as usize],
                        p.image.as_raw(),
                    ),
                    egui::TextureOptions::LINEAR,
                ));
            }
            PasteView {
                tex: p.tex.as_ref().map(|h| h.id()).unwrap_or_default(),
                size: Vec2::new(p.image.width() as f32, p.image.height() as f32),
                shape: p.shape,
            }
        });
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
        let px_per_pt = img_w as f32 / size.x.max(1.0);
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        egui::ScrollArea::both()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let canvas = size.max(avail.size());
                let (canvas_rect, _) = ui.allocate_exact_size(canvas, Sense::hover());
                let img_rect = Rect::from_center_size(canvas_rect.center(), size);
                ui.painter().image(tex, img_rect, uv, egui::Color32::WHITE);
                // A floating paste takes over the canvas: drag to
                // position, click-outside/Enter flattens, Esc discards.
                if let Some(view) = pasted {
                    self.paste_layer(ui, index, img_rect, px_per_pt, &view);
                } else {
                    self.marquee(ui, img_rect, px_per_pt);
                }
            });
    }

    /// Rectangle tool: marquee drag on the canvas (coordinates mapped back
    /// to image physical px), dashed accent border, double-click crops.
    fn marquee(&mut self, ui: &mut egui::Ui, img_rect: Rect, px_per_pt: f32) {
        if !self.select_tool {
            // A live selection still paints — the tool can be disarmed while
            // a pending crop choice remains.
        } else {
            let resp = ui.interact(
                img_rect,
                ui.id().with("pixme-viewer-marquee"),
                Sense::click_and_drag(),
            );
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if resp.drag_started()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                self.marquee_anchor = Some(image_pos(pos, img_rect, px_per_pt));
                self.selection = None;
                self.selection_shape = None;
            }
            if resp.dragged()
                && let (Some(anchor), Some(pos)) =
                    (self.marquee_anchor, resp.interact_pointer_pos())
            {
                let cur = image_pos(pos, img_rect, px_per_pt);
                self.selection = Some(Rect::from_two_pos(anchor, cur));
                self.selection_shape = Some(self.select_shape);
            }
            if resp.drag_stopped() {
                self.marquee_anchor = None;
                // Keep only a real drag — a stray click clears instead.
                if self
                    .selection
                    .is_some_and(|s| s.width() < 2.0 || s.height() < 2.0)
                {
                    self.selection = None;
                    self.selection_shape = None;
                }
            }
            if resp.double_clicked()
                && let (Some(sel), Some(pos)) = (self.selection, resp.interact_pointer_pos())
                && sel.contains(image_pos(pos, img_rect, px_per_pt))
            {
                self.crop_active(sel);
                return;
            }
        }
        let Some(sel) = self.selection else { return };
        let canvas_sel = Rect::from_min_size(
            img_rect.min + sel.min.to_vec2() / px_per_pt,
            sel.size() / px_per_pt,
        );
        let t = theme::tokens(ui.ctx());
        marquee_outline(ui.painter(), canvas_sel, self.select_shape, t.accent);
    }

    /// The floating paste on tab `index`: painted at its position with
    /// the marquee's chrome in the shape it was copied with, draggable
    /// to reposition. Any press outside flattens it onto the document.
    fn paste_layer(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        img_rect: Rect,
        px_per_pt: f32,
        view: &PasteView,
    ) {
        let tab = &self.tabs[index];
        let (doc_w, doc_h) = (
            tab.doc.base.image.width() as f32,
            tab.doc.base.image.height() as f32,
        );
        let pos = tab.pasted.as_ref().map(|p| p.pos).unwrap_or_default();
        let layer_rect = Rect::from_min_size(
            img_rect.min + pos.to_vec2() / px_per_pt,
            view.size / px_per_pt,
        );
        // A press landing outside the layer flattens it (toolbar clicks
        // included — the layer's own drag is exempt).
        let outside = ui.input(|i| {
            i.pointer.primary_pressed()
                && i.pointer
                    .press_origin()
                    .is_some_and(|p| !layer_rect.contains(p))
        });
        if outside {
            self.commit_paste(index);
            return;
        }
        let resp = ui.interact(
            layer_rect,
            ui.id().with("pixme-pasted-layer"),
            Sense::drag(),
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
        }
        if resp.dragged()
            && let Some(p) = &mut self.tabs[index].pasted
        {
            // Screen-pt drag delta → image-physical px, clamped so the
            // layer stays inside the document.
            let delta = resp.drag_delta() * px_per_pt;
            p.pos.x = (p.pos.x + delta.x).clamp(0.0, (doc_w - view.size.x).max(0.0));
            p.pos.y = (p.pos.y + delta.y).clamp(0.0, (doc_h - view.size.y).max(0.0));
        }
        let t = theme::tokens(ui.ctx());
        let painter = ui.painter();
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(view.tex, layer_rect, uv, egui::Color32::WHITE);
        // Same chrome as the marquee, in the copied shape: an oval
        // paste reads as an oval, not its bounding rect.
        marquee_outline(painter, layer_rect, view.shape, t.accent);
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

    /// The rename prompt (File → Rename): a title field over a window.
    /// The draft is taken out for the frame; on Apply the tab is renamed,
    /// on Cancel/close the draft is dropped.
    fn rename_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.rename.take() else {
            return;
        };
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
                ui.add(
                    egui::TextEdit::singleline(&mut draft)
                        .desired_width(200.0)
                        .font(theme::medium(12.0)),
                );
            });
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
            let name = draft.trim();
            if !name.is_empty() {
                self.set_active_title(name.to_owned());
            }
        } else if !cancel {
            self.rename = Some(draft);
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

/// Pointer position in canvas space → image physical px, clamped to the
/// image bounds.
fn image_pos(pos: Pos2, img_rect: Rect, px_per_pt: f32) -> Pos2 {
    let px = (pos - img_rect.min) * px_per_pt;
    Pos2::new(px.x.max(0.0), px.y.max(0.0))
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

/// Rectangle's shape menu — also arms the tool.
fn shape_menu(ui: &egui::Ui, resp: &Response, v: &mut Viewer, open: bool) {
    tool_popup(ui, resp, "shape-menu", open, |ui| {
        for (shape, key) in [
            (SelectShape::Rect, "vbar.select.rectangle"),
            (SelectShape::Oval, "vbar.select.oval"),
            (SelectShape::Round, "vbar.select.rounded"),
        ] {
            if ui
                .selectable_label(v.select_shape == shape, t!(key))
                .clicked()
            {
                v.select_shape = shape;
                v.select_tool = true;
                ui.close();
            }
        }
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

/// The marquee chrome — a white halo under the accent outline so it
/// stays readable on arbitrary screenshots — in the requested shape.
/// Shared by the live marquee and the floating paste's outline.
fn marquee_outline(painter: &egui::Painter, rect: Rect, shape: SelectShape, accent: egui::Color32) {
    match shape {
        SelectShape::Rect => {
            painter.rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(2.0, egui::Color32::WHITE),
                StrokeKind::Outside,
            );
            let a = rect.min;
            let b = rect.max;
            let stroke = Stroke::new(1.0, accent);
            // `dashed_line` yields a Vec<Shape>: extend per edge.
            for (p, q) in [
                (a, Pos2::new(b.x, a.y)),
                (Pos2::new(b.x, a.y), b),
                (b, Pos2::new(a.x, b.y)),
                (Pos2::new(a.x, b.y), a),
            ] {
                painter.extend(egui::Shape::dashed_line(&[p, q], stroke, 6.0, 4.0));
            }
        }
        SelectShape::Round => {
            let radii = CornerRadius::same(8);
            painter.rect_stroke(
                rect,
                radii,
                Stroke::new(3.0, egui::Color32::WHITE),
                StrokeKind::Middle,
            );
            painter.rect_stroke(rect, radii, Stroke::new(1.5, accent), StrokeKind::Middle);
        }
        SelectShape::Oval => {
            let outline = |stroke: Stroke| {
                egui::Shape::from(egui::epaint::EllipseShape {
                    center: rect.center(),
                    radius: rect.size() * 0.5,
                    fill: egui::Color32::TRANSPARENT,
                    stroke,
                    angle: 0.0,
                })
            };
            painter.add(outline(Stroke::new(3.0, egui::Color32::WHITE)));
            painter.add(outline(Stroke::new(1.5, accent)));
        }
    }
}

/// The outline a pasted layer gets. A bitmap with opaque corners is
/// solid content — it reads as a plain rect whatever was recorded.
/// Transparent corners mean a shaped copy: the recorded shape wins, or
/// (external content, nothing recorded) the top edge is probed — a
/// rounded rect turns opaque past its corner radius while an ellipse
/// stays transparent until the curve peaks.
fn paste_outline_shape(img: &image::RgbaImage, recorded: Option<SelectShape>) -> SelectShape {
    let (w, h) = (img.width(), img.height());
    if w < 2 || h < 2 {
        return SelectShape::Rect;
    }
    let opaque = |x: u32, y: u32| img.get_pixel(x.min(w - 1), y.min(h - 1)).0[3] > 127;
    if [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)]
        .into_iter()
        .all(|(x, y)| opaque(x, y))
    {
        return SelectShape::Rect;
    }
    match recorded {
        Some(shape @ (SelectShape::Oval | SelectShape::Round)) => shape,
        _ => {
            if opaque(w / 4, 0) {
                SelectShape::Round
            } else {
                SelectShape::Oval
            }
        }
    }
}

/// Clamps a selection rect to image bounds (physical px).
fn clamp_to_image(sel: Rect, w: u32, h: u32) -> Rect {
    let bounds = Rect::from_min_size(Pos2::ZERO, Vec2::new(w as f32, h as f32));
    sel.intersect(bounds)
}

/// Pixels outside the inscribed ellipse go transparent (oval-selection
/// copy — the paste composites them back over the document).
fn mask_ellipse(img: &mut image::RgbaImage) {
    let (w, h) = (img.width() as f64, img.height() as f64);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (rx, ry, cx, cy) = (w / 2.0, h / 2.0, w / 2.0, h / 2.0);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let dx = (x as f64 + 0.5 - cx) / rx;
        let dy = (y as f64 + 0.5 - cy) / ry;
        if dx * dx + dy * dy > 1.0 {
            p.0[3] = 0;
        }
    }
}

/// Pixels outside the rounded rect go transparent — signed distance to
/// the shrunk inner rect vs the corner radius.
fn mask_rounded(img: &mut image::RgbaImage, radius: f64) {
    let (w, h) = (img.width() as f64, img.height() as f64);
    let r = radius.min(w / 2.0).min(h / 2.0);
    if r <= 0.0 {
        return;
    }
    for (x, y, p) in img.enumerate_pixels_mut() {
        let px = x as f64 + 0.5;
        let py = y as f64 + 0.5;
        // Nearest point of the shrunk rect; corners read as a circle.
        let qx = px.clamp(r, w - r);
        let qy = py.clamp(r, h - r);
        let (dx, dy) = (px - qx, py - qy);
        if dx * dx + dy * dy > r * r {
            p.0[3] = 0;
        }
    }
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

    /// A tab backed by an `n × n` opaque document (big enough to mask).
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
            pasted: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
        });
        v.active = v.tabs.len() - 1;
    }

    #[test]
    fn oval_selection_copies_transparent_corners() {
        let mut v = Viewer::default();
        push_doc(&mut v, 10, 10);
        v.selection = Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(10.0, 10.0)));
        v.selection_shape = Some(SelectShape::Oval);
        let img = v.copy_image().expect("copy");
        assert_eq!(img.get_pixel(0, 0)[3], 0, "corner must be transparent");
        assert_eq!(img.get_pixel(9, 9)[3], 0, "corner must be transparent");
        assert_eq!(img.get_pixel(5, 5)[3], 255, "centre stays opaque");
    }

    #[test]
    fn rounded_selection_copies_transparent_corners() {
        let mut v = Viewer::default();
        push_doc(&mut v, 20, 20);
        v.selection = Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0)));
        v.selection_shape = Some(SelectShape::Round);
        let img = v.copy_image().expect("copy");
        assert_eq!(img.get_pixel(0, 0)[3], 0, "corner must be transparent");
        assert_eq!(img.get_pixel(10, 10)[3], 255, "centre stays opaque");
    }

    #[test]
    fn rect_selection_copies_opaque() {
        let mut v = Viewer::default();
        push_doc(&mut v, 10, 10);
        v.selection = Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(10.0, 10.0)));
        v.selection_shape = Some(SelectShape::Rect);
        let img = v.copy_image().expect("copy");
        assert_eq!(img.get_pixel(0, 0)[3], 255, "rect keeps its corners");
    }

    #[test]
    fn repeat_paste_keeps_copied_shape() {
        let mut v = Viewer::default();
        push_doc(&mut v, 10, 10);
        v.selection = Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(10.0, 10.0)));
        v.selection_shape = Some(SelectShape::Oval);
        let img = v.copy_image().expect("copy");
        // Two Coller in a row: the copied shape survives the first drop.
        assert!(v.paste_active(img.clone()));
        assert_eq!(
            v.tabs[0].pasted.as_ref().map(|p| p.shape),
            Some(SelectShape::Oval)
        );
        assert!(v.paste_active(img));
        assert_eq!(
            v.tabs[0].pasted.as_ref().map(|p| p.shape),
            Some(SelectShape::Oval)
        );
    }

    #[test]
    fn external_solid_paste_outlines_rect() {
        let mut v = Viewer::default();
        push_doc(&mut v, 10, 10);
        // A stale copied shape must not dress up opaque-cornered
        // (external) content — it reads as a plain rectangle.
        v.copy_shape = Some(SelectShape::Oval);
        let solid = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        assert!(v.paste_active(solid));
        assert_eq!(
            v.tabs[0].pasted.as_ref().map(|p| p.shape),
            Some(SelectShape::Rect)
        );
    }

    #[test]
    fn undo_redo_flatten_restores_pixels() {
        let mut v = Viewer::default();
        push_doc(&mut v, 10, 10);
        let layer = image::RgbaImage::from_pixel(4, 4, image::Rgba([250, 250, 250, 255]));
        assert!(v.paste_active(layer));
        v.commit_paste(0);
        assert_eq!(v.tabs[0].doc.base.image.get_pixel(0, 0)[0], 250);
        // Undo restores the covered patch; everything else untouched.
        assert!(v.undo_active());
        assert_eq!(
            v.tabs[0].doc.base.image.get_pixel(0, 0).0,
            [10, 20, 30, 255]
        );
        assert_eq!(
            v.tabs[0].doc.base.image.get_pixel(9, 9).0,
            [10, 20, 30, 255]
        );
        // Redo re-applies the same composite.
        assert!(v.redo_active());
        assert_eq!(v.tabs[0].doc.base.image.get_pixel(0, 0)[0], 250);
    }

    #[test]
    fn undo_redo_crop_restores_dimensions() {
        let mut v = Viewer::default();
        push_doc(&mut v, 20, 20);
        v.crop_active(Rect::from_min_max(
            Pos2::new(2.0, 2.0),
            Pos2::new(10.0, 10.0),
        ));
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
    fn undo_discards_floating_paste_first() {
        let mut v = Viewer::default();
        push_doc(&mut v, 10, 10);
        assert!(v.paste_active(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([9, 9, 9, 255])
        )));
        // ⌘Z while a layer floats abandons it — the doc never changed.
        assert!(v.undo_active());
        assert!(v.tabs[0].pasted.is_none());
        assert_eq!(
            v.tabs[0].doc.base.image.get_pixel(0, 0).0,
            [10, 20, 30, 255]
        );
        assert!(!v.undo_active());
    }
}

#[cfg(test)]
mod clipboard_chain_tests {
    use super::*;

    /// Full Copy → OS clipboard → Coller chain, minus egui rendering:
    /// the floating layer must carry the selection's transparency.
    #[test]
    #[ignore = "touches the real OS clipboard"]
    fn oval_copy_paste_keeps_transparency() {
        let backup = crate::export::clipboard::clipboard_image().ok().flatten();
        let mut v = Viewer::default();
        v.tabs.push(Tab {
            title: "t".to_owned(),
            doc: Document::new(crate::capture::Capture {
                image: image::RgbaImage::from_pixel(20, 20, image::Rgba([10, 20, 30, 255])),
                scale_factor: 1.0,
                monitor: crate::capture::MonitorInfo {
                    id: 0,
                    name: String::new(),
                    rect: crate::capture::PhysicalRect {
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
            pasted: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
        });
        v.active = 0;
        v.selection = Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0)));
        v.selection_shape = Some(SelectShape::Oval);
        // The exact app path: CopyActive → copy_image → clipboard.
        let img = v.copy_image().expect("copy");
        crate::export::clipboard::copy_image(&img).expect("set clipboard");
        // …then Coller → clipboard_image → paste_active.
        let back = crate::export::clipboard::clipboard_image()
            .expect("get clipboard")
            .expect("image on clipboard");
        assert!(v.paste_active(back));
        let layer = v.tabs[0].pasted.as_ref().expect("floating layer");
        assert_eq!(
            layer.image.get_pixel(0, 0)[3],
            0,
            "pasted layer lost the selection's transparency"
        );
        if let Some(img) = backup {
            let _ = crate::export::clipboard::copy_image(&img);
        }
    }
}
