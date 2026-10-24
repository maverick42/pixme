//! The full-screen region-selection overlay (functional reference's capture
//! crosshair): the frozen monitor frame dimmed, a resizable selection with
//! eight handles, a magnifier following the cursor (flipped away from screen
//! edges), the selection size, the pixel colour under the cursor, and
//! "Press C to copy".
//!
//! Coordinates: the overlay window covers one monitor, so widget space is the
//! monitor's *logical* space; the frozen frame and the confirmed selection
//! are in *physical* pixels (`logical × scale_factor`).

use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2, ViewportCommand};
use rust_i18n::t;

use crate::app::{AppState, CaptureMode, Command, Shared};
use crate::capture::screen;
use crate::capture::{Capture, MonitorInfo, PhysicalRect};
use crate::ui::theme;

/// Veil over the frozen frame outside the selection.
const DIM: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 110);
/// Selection border / crosshair colour (theme-independent capture chrome).
const ACCENT: Color32 = Color32::from_rgb(0x37, 0x8E, 0xF0);
/// Magnifier centre lines (the reference's green reticle).
const LOUPE_CROSS: Color32 = Color32::from_rgb(0x2F, 0x9E, 0x44);
/// Handle radius (corners) and half-size (edge pills), logical px.
const HANDLE_R: f32 = 6.0;
/// Extra logical px around a handle that still grabs it.
const HANDLE_HIT: f32 = 11.0;
/// Magnifier side, logical px.
const LOUPE_SIDE: f32 = 176.0;
/// Source pixels sampled around the cursor (odd → cursor centred).
const LOUPE_SAMPLE: i32 = 21;
/// Logical px between cursor and the magnifier card.
const LOUPE_GAP: f32 = 24.0;
/// Default fixed-region size, logical px (stays resizable via the handles).
const FIXED_SIZE: Vec2 = Vec2::new(640.0, 480.0);
/// "Copied" feedback duration.
const COPIED_SECS: f32 = 1.2;

/// One of the eight resize handles.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Handle {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
}

/// What the primary-button drag is doing.
#[derive(Clone, Copy)]
enum Drag {
    /// Drawing a new selection from `anchor`.
    Create(Pos2),
    /// Moving the selection; `offset` is cursor minus `selection.min`.
    Move(Vec2),
    /// Resizing through one handle.
    Resize(Handle),
    /// Fixed-region mode: the frame follows the cursor until first click.
    Place,
}

/// Live selection session stored in [`Shared::capture`] while `Capturing`.
pub struct OverlaySession {
    /// Which capture started the overlay.
    pub mode: CaptureMode,
    /// Where to land on cancel (the window the capture was launched from).
    pub return_to: AppState,
    /// Whole-screen modes freeze and confirm without any selection UI.
    pub instant: bool,
    /// Frames painted before the screenshot is taken — the bar's window must
    /// be torn down first or it would photograph itself.
    warm_frames: u8,
    /// The frozen monitor frame (physical px), once armed.
    pub frozen: Option<Capture>,
    /// Monitor being covered.
    pub monitor: Option<MonitorInfo>,
    /// Current selection in overlay-logical coordinates.
    pub selection: Option<Rect>,
    /// Active drag, if any.
    drag: Option<Drag>,
    /// Fixed-region mode: the frame follows the cursor until placed.
    placed: bool,
    /// Window origin minus monitor origin, in logical points: macOS clamps
    /// new windows into the *visible* frame (screen minus menu bar / Dock),
    /// so the window may sit below the monitor's true top. The frozen frame
    /// is painted offset by this so that screen point == image point, and
    /// every window-local position gets it added when mapped to physical px.
    screen_offset: Vec2,
    /// Whether the post-creation fullscreen nudge was sent (macOS clamps at
    /// init; a programmatic setFrame may escape it).
    nudged: bool,
    /// Full-screen frozen frame texture.
    texture: Option<egui::TextureHandle>,
    /// Magnifier source texture (rebuilt around the cursor each frame).
    loupe: Option<egui::TextureHandle>,
    /// When `C` last copied the colour (drives the "copied" flash).
    copied_at: Option<std::time::Instant>,
}

impl OverlaySession {
    /// A fresh session for `mode`; `return_to` is where Esc lands.
    pub fn new(mode: CaptureMode, return_to: AppState) -> Self {
        Self {
            mode,
            return_to,
            instant: matches!(mode, CaptureMode::FullScreen),
            warm_frames: 0,
            frozen: None,
            monitor: None,
            selection: None,
            drag: None,
            placed: !matches!(mode, CaptureMode::FixedRegion),
            screen_offset: Vec2::ZERO,
            nudged: false,
            texture: None,
            loupe: None,
            copied_at: None,
        }
    }

    /// Freeze warm-up, called by the **anchor** pass while no capture window
    /// exists yet: two anchor frames let the floating bar finish tearing
    /// down, then the screenshot is taken. Crucially this runs *before* the
    /// overlay viewport is created — capturing while it is on screen would
    /// photograph the overlay's own veil. Returns `true` once frozen.
    pub fn warm_freeze(&mut self) -> bool {
        self.warm_frames += 1;
        if self.warm_frames < 2 {
            return false;
        }
        if self.frozen.is_none() {
            match screen::primary_monitor().and_then(|m| screen::capture_monitor(m.id)) {
                Ok(frame) => {
                    // Dev hook: PIXME_DUMP_FROZEN=/path.png dumps the frozen frame.
                    if let Some(path) = std::env::var_os("PIXME_DUMP_FROZEN") {
                        let _ = frame.image.save(&path);
                    }
                    self.monitor = Some(frame.monitor.clone());
                    self.frozen = Some(frame);
                }
                Err(err) => tracing::error!("capture failed: {err}"),
            }
        }
        self.frozen.is_some()
    }

    /// The overlay pass — draws everything and consumes input.
    pub fn show(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            shared.push(Command::CancelCapture);
            return;
        }
        let rect = ui.max_rect();
        let t = theme::tokens(&ctx);
        ui.painter().rect_filled(rect, 0.0, t.dock);

        if self.frozen.is_none() {
            // The anchor only registers this viewport once the frame is
            // frozen — reaching here means the session was swapped out
            // mid-frame; paint the bare veil and wait for cancel.
            ctx.request_repaint();
            return;
        }

        // -- align the frozen frame to the real screen --------------------
        // Track where the window actually landed vs the monitor origin and
        // retry the true full-screen frame once (init clamps; setFrame may
        // not). Either way `screen_offset` keeps the mapping exact.
        if let (Some(outer), Some(m)) =
            (ctx.input(|i| i.viewport().outer_rect), self.monitor.clone())
        {
            self.screen_offset = outer.min.to_vec2() - egui::vec2(m.rect.x as f32, m.rect.y as f32);
            if !self.nudged {
                self.nudged = true;
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(
                    m.rect.x as f32,
                    m.rect.y as f32,
                )));
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(
                    m.rect.width as f32,
                    m.rect.height as f32,
                )));
            }
        }

        // -- the frozen frame, dimmed outside the selection ---------------
        let tex = self.texture.get_or_insert_with(|| {
            let img = self
                .frozen
                .as_ref()
                .map(|f| &f.image)
                .expect("checked above");
            ctx.load_texture(
                "frozen-frame",
                egui::ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    img.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            )
        });
        // The frame is painted shifted by `-screen_offset` so it stays in
        // sync with the (possibly clamped) physical screen.
        let image_rect = rect.translate(-self.screen_offset);
        let full_uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        ui.painter()
            .image(tex.id(), image_rect, full_uv, Color32::WHITE);
        if let Some(sel) = self.selection {
            // Four dimmed bands around the untouched selection.
            for band in [
                Rect::from_min_max(rect.min, Pos2::new(rect.max.x, sel.min.y)),
                Rect::from_min_max(Pos2::new(rect.min.x, sel.max.y), rect.max),
                Rect::from_min_max(
                    Pos2::new(rect.min.x, sel.min.y),
                    Pos2::new(sel.min.x, sel.max.y),
                ),
                Rect::from_min_max(
                    Pos2::new(sel.max.x, sel.min.y),
                    Pos2::new(rect.max.x, sel.max.y),
                ),
            ] {
                ui.painter().rect_filled(band, 0.0, DIM);
            }
        } else {
            ui.painter().rect_filled(rect, 0.0, DIM);
        }

        // -- input ---------------------------------------------------------
        let resp = ui.interact(rect, ui.id().with("overlay"), Sense::click_and_drag());
        // Fixed-region: the frame follows the cursor until a press places it.
        if !self.placed {
            if let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) {
                self.selection = Some(slide_inside(Rect::from_center_size(pos, FIXED_SIZE), rect));
            }
            if resp.drag_started() || resp.clicked() {
                self.placed = true;
            }
        }
        if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
            self.handle_drag(pos, &resp, rect);
        }
        if resp.double_clicked() && self.selection.is_some() {
            shared.push(Command::ConfirmCapture);
        }
        self.keys(&ctx, shared);
        self.paint(ui, &ctx);
        ctx.request_repaint(); // crosshair + loupe track every move
    }

    /// Pointer input: hit-test handles → inside → outside, update selection.
    /// The selection never leaves `bounds` (the window = the visible monitor
    /// area): out-of-window drags stop at the edges instead of overrunning
    /// the menu bar or the screen borders.
    fn handle_drag(&mut self, pos: Pos2, resp: &egui::Response, bounds: Rect) {
        let pos = clamp_pos(pos, bounds);
        if resp.drag_started() {
            self.drag = Some(if let Some(sel) = self.selection {
                if let Some(h) = hit_handle(sel, pos) {
                    Drag::Resize(h)
                } else if sel.contains(pos) {
                    Drag::Move(pos - sel.min)
                } else {
                    Drag::Create(pos)
                }
            } else {
                match self.mode {
                    // Fixed-region: the frame rides the cursor until clicked.
                    CaptureMode::FixedRegion => Drag::Place,
                    _ => Drag::Create(pos),
                }
            });
        }
        let Some(drag) = self.drag else { return };
        match drag {
            Drag::Create(anchor) => {
                self.selection = Some(clamp_inside(Rect::from_two_pos(anchor, pos), bounds));
            }
            Drag::Move(offset) => {
                if let Some(sel) = self.selection {
                    let size = sel.size();
                    self.selection = Some(slide_inside(
                        Rect::from_min_size(pos - offset, size),
                        bounds,
                    ));
                }
            }
            Drag::Place => {
                self.selection = Some(slide_inside(
                    Rect::from_center_size(pos, FIXED_SIZE),
                    bounds,
                ));
            }
            Drag::Resize(h) => {
                if let Some(sel) = self.selection {
                    self.selection = Some(clamp_inside(resize(sel, h, pos), bounds));
                }
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
            if let Some(sel) = self.selection {
                // Shrink-to-nothing counts as no selection.
                if sel.width() < 2.0 || sel.height() < 2.0 {
                    self.selection = None;
                }
            }
        }
    }

    /// Esc cancels, Enter confirms, C copies the cursor colour.
    fn keys(&mut self, ctx: &egui::Context, shared: &Shared) {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            shared.push(Command::CancelCapture);
            return;
        }
        if self.selection.is_some()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        {
            shared.push(Command::ConfirmCapture);
            return;
        }
        if ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::C)
                || i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
        }) && let Some(hex) = self.cursor_hex(ctx)
            && let Ok(mut cb) = arboard::Clipboard::new()
        {
            let _ = cb.set_text(&hex);
            self.copied_at = Some(std::time::Instant::now());
        }
    }

    /// Selection, crosshair, magnifier.
    fn paint(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let rect = ui.max_rect();
        if let Some(sel) = self.selection {
            paint_selection(ui, sel);
        }
        let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        if !rect.contains(pos) {
            return;
        }
        // Crosshair: thin accent lines across the monitor through the cursor.
        if self.selection.is_none() && self.drag.is_none() {
            let stroke = Stroke::new(1.0, ACCENT);
            ui.painter().hline(rect.x_range(), pos.y, stroke);
            ui.painter().vline(pos.x, rect.y_range(), stroke);
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        } else {
            ctx.set_cursor_icon(cursor_for(self.selection, pos, self.drag));
        }
        self.loupe(ui, ctx, pos);
    }

    /// The magnifier card: zoomed pixels + size + hex colour + copy hint,
    /// flipped to whichever side of the cursor keeps it on screen.
    fn loupe(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, pos: Pos2) {
        let rect = ui.max_rect();
        let scale = self.frozen.as_ref().map(|f| f.scale_factor).unwrap_or(1.0);
        let text_h = 78.0_f32;
        let size = Vec2::new(LOUPE_SIDE, LOUPE_SIDE + text_h);
        // Prefer below-right of the cursor; flip near edges.
        let mut min = pos + Vec2::splat(LOUPE_GAP);
        if min.x + size.x > rect.max.x - 8.0 {
            min.x = pos.x - LOUPE_GAP - size.x;
        }
        if min.y + size.y > rect.max.y - 8.0 {
            min.y = pos.y - LOUPE_GAP - size.y;
        }
        let card = Rect::from_min_size(min, size);
        let zoom_rect = Rect::from_min_size(card.min, Vec2::splat(LOUPE_SIDE));
        let text_rect = Rect::from_min_max(Pos2::new(card.min.x, zoom_rect.max.y), card.max);
        ui.painter()
            .rect_filled(card, 8.0, Color32::from_black_alpha(235));
        ui.painter().rect_stroke(
            card,
            8.0,
            Stroke::new(1.0, Color32::from_gray(70)),
            egui::StrokeKind::Outside,
        );

        // Zoomed pixels around the cursor (physical px → NEAREST texture).
        if let Some(sample) = self.sample_loupe(pos, scale) {
            let zoomed = egui::ColorImage::from_rgba_unmultiplied(
                [LOUPE_SAMPLE as usize, LOUPE_SAMPLE as usize],
                &sample,
            );
            let tex = self.loupe.get_or_insert_with(|| {
                ctx.load_texture(
                    "loupe",
                    egui::ColorImage::filled(
                        [LOUPE_SAMPLE as usize, LOUPE_SAMPLE as usize],
                        Color32::BLACK,
                    ),
                    egui::TextureOptions::NEAREST,
                )
            });
            tex.set(zoomed, egui::TextureOptions::NEAREST);
            let painter = ui.painter_at(zoom_rect);
            painter.image(tex.id(), zoom_rect, full_uv(), Color32::WHITE);
            let centre = zoom_rect.center();
            painter.hline(zoom_rect.x_range(), centre.y, Stroke::new(1.0, LOUPE_CROSS));
            painter.vline(centre.x, zoom_rect.y_range(), Stroke::new(1.0, LOUPE_CROSS));
            // Hairline separating zoom from the text block.
            painter.hline(
                card.x_range(),
                text_rect.min.y,
                Stroke::new(1.0, Color32::from_gray(70)),
            );
        }

        // Text block: size or coords, hex colour, copy hint.
        let painter = ui.painter_at(text_rect);
        let font = egui::FontId::monospace(13.0);
        let centre_x = text_rect.center().x;
        let mut y = text_rect.min.y + 14.0;
        let line1 = match self.selection {
            Some(sel) => {
                let phys = sel.size() * scale;
                format!("{:.0} * {:.0}", phys.x.max(0.0), phys.y.max(0.0))
            }
            None => {
                let p = pos + self.screen_offset;
                format!("{:.0}, {:.0}", p.x * scale, p.y * scale)
            }
        };
        for (text, color) in [
            (line1, Color32::WHITE),
            (
                self.cursor_hex(ctx)
                    .unwrap_or_else(|| "#------".to_string()),
                Color32::WHITE,
            ),
            (
                if self
                    .copied_at
                    .is_some_and(|t| t.elapsed().as_secs_f32() < COPIED_SECS)
                {
                    t!("overlay.copied").to_string()
                } else {
                    t!("overlay.copy_hint").to_string()
                },
                Color32::from_gray(160),
            ),
        ] {
            painter.text(
                Pos2::new(centre_x, y),
                egui::Align2::CENTER_TOP,
                text,
                font.clone(),
                color,
            );
            y += 20.0;
        }
    }

    /// Raw RGBA bytes of the `LOUPE_SAMPLE`² area centred on the cursor.
    fn sample_loupe(&self, pos: Pos2, scale: f32) -> Option<Vec<u8>> {
        let img = &self.frozen.as_ref()?.image;
        let (w, h) = (img.width() as i32, img.height() as i32);
        let p = pos + self.screen_offset;
        let cx = (p.x * scale) as i32;
        let cy = (p.y * scale) as i32;
        let half = LOUPE_SAMPLE / 2;
        let mut out = Vec::with_capacity((LOUPE_SAMPLE * LOUPE_SAMPLE * 4) as usize);
        for dy in -half..=half {
            for dx in -half..=half {
                let (x, y) = (cx + dx, cy + dy);
                let px = if x >= 0 && y >= 0 && x < w && y < h {
                    img.get_pixel(x as u32, y as u32).0
                } else {
                    [0, 0, 0, 255]
                };
                out.extend_from_slice(&px);
            }
        }
        Some(out)
    }

    /// `#RRGGBB` of the physical pixel under the cursor.
    fn cursor_hex(&self, ctx: &egui::Context) -> Option<String> {
        let img = &self.frozen.as_ref()?.image;
        let pos = ctx.input(|i| i.pointer.latest_pos())?;
        let scale = self.frozen.as_ref()?.scale_factor;
        let p = pos + self.screen_offset;
        let (x, y) = ((p.x * scale) as i64, (p.y * scale) as i64);
        if x < 0 || y < 0 || x >= img.width() as i64 || y >= img.height() as i64 {
            return None;
        }
        let p = img.get_pixel(x as u32, y as u32).0;
        Some(format!("#{:02X}{:02X}{:02X}", p[0], p[1], p[2]))
    }

    /// Crops the selection (physical px) out of the frozen frame.
    pub fn take_capture(&self) -> Option<Capture> {
        let frozen = self.frozen.as_ref()?;
        let sel = self.selection?;
        let s = frozen.scale_factor;
        let min = sel.min.to_vec2() + self.screen_offset;
        let rect = PhysicalRect {
            x: (min.x * s).round() as i32,
            y: (min.y * s).round() as i32,
            width: (sel.width() * s).round().max(1.0) as u32,
            height: (sel.height() * s).round().max(1.0) as u32,
        };
        Some(screen::crop_region(frozen, rect))
    }
}

/// The dashed accent border + eight handles of the live selection.
fn paint_selection(ui: &mut egui::Ui, sel: Rect) {
    let painter = ui.painter();
    let stroke = Stroke::new(1.5, ACCENT);
    for (a, b) in [
        (sel.min, sel.right_top()),
        (sel.right_top(), sel.max),
        (sel.max, sel.left_bottom()),
        (sel.left_bottom(), sel.min),
    ] {
        painter.extend(egui::Shape::dashed_line(&[a, b], stroke, 6.0, 4.0));
    }
    for h in [
        Handle::Nw,
        Handle::N,
        Handle::Ne,
        Handle::E,
        Handle::Se,
        Handle::S,
        Handle::Sw,
        Handle::W,
    ] {
        let centre = handle_centre(sel, h);
        let is_edge = matches!(h, Handle::N | Handle::E | Handle::S | Handle::W);
        if is_edge {
            let pill = Rect::from_center_size(centre, Vec2::new(HANDLE_R * 2.6, HANDLE_R * 1.4));
            painter.rect_filled(pill, pill.height() / 2.0, Color32::WHITE);
            painter.rect_stroke(pill, pill.height() / 2.0, stroke, egui::StrokeKind::Outside);
        } else {
            painter.circle_filled(centre, HANDLE_R, Color32::WHITE);
            painter.circle_stroke(centre, HANDLE_R, stroke);
        }
    }
}

/// Centre point of a handle on `sel`.
fn handle_centre(sel: Rect, h: Handle) -> Pos2 {
    let (l, r, t, b) = (sel.min.x, sel.max.x, sel.min.y, sel.max.y);
    let (cx, cy) = (sel.center().x, sel.center().y);
    match h {
        Handle::Nw => Pos2::new(l, t),
        Handle::N => Pos2::new(cx, t),
        Handle::Ne => Pos2::new(r, t),
        Handle::E => Pos2::new(r, cy),
        Handle::Se => Pos2::new(r, b),
        Handle::S => Pos2::new(cx, b),
        Handle::Sw => Pos2::new(l, b),
        Handle::W => Pos2::new(l, cy),
    }
}

/// Clamps a pointer position to `bounds` (window-local).
fn clamp_pos(pos: Pos2, bounds: Rect) -> Pos2 {
    Pos2::new(
        pos.x.clamp(bounds.min.x, bounds.max.x),
        pos.y.clamp(bounds.min.y, bounds.max.y),
    )
}

/// Shrinks `sel` to the part inside `bounds` (create/resize drags stop at
/// the screen edges); a fully-outside drag collapses to an empty rect that
/// `drag_stopped` drops.
fn clamp_inside(sel: Rect, bounds: Rect) -> Rect {
    let s = sel.intersect(bounds);
    if s.is_positive() {
        s
    } else {
        Rect::from_center_size(clamp_pos(sel.center(), bounds), Vec2::ZERO)
    }
}

/// Slides `sel` fully inside `bounds` keeping its size (move drags and the
/// fixed-size frame): the rect hugs the edge instead of overrunning it.
fn slide_inside(sel: Rect, bounds: Rect) -> Rect {
    let size = sel.size().min(bounds.size());
    let min = Pos2::new(
        sel.min.x.clamp(bounds.min.x, bounds.max.x - size.x),
        sel.min.y.clamp(bounds.min.y, bounds.max.y - size.y),
    );
    Rect::from_min_size(min, size)
}

/// Which handle (if any) sits under `pos`.
fn hit_handle(sel: Rect, pos: Pos2) -> Option<Handle> {
    [
        Handle::Nw,
        Handle::N,
        Handle::Ne,
        Handle::E,
        Handle::Se,
        Handle::S,
        Handle::Sw,
        Handle::W,
    ]
    .into_iter()
    .find(|&h| handle_centre(sel, h).distance(pos) <= HANDLE_HIT)
}

/// Applies a handle resize, keeping `min <= max` (dragging a handle across
/// the opposite edge mirrors the selection).
fn resize(sel: Rect, h: Handle, pos: Pos2) -> Rect {
    let mut min = sel.min;
    let mut max = sel.max;
    match h {
        Handle::N | Handle::Nw | Handle::Ne => min.y = pos.y,
        Handle::S | Handle::Sw | Handle::Se => max.y = pos.y,
        _ => {}
    }
    match h {
        Handle::W | Handle::Nw | Handle::Sw => min.x = pos.x,
        Handle::E | Handle::Ne | Handle::Se => max.x = pos.x,
        _ => {}
    }
    Rect::from_min_max(min, max)
}

/// Cursor icon for the current hover zone.
fn cursor_for(selection: Option<Rect>, pos: Pos2, drag: Option<Drag>) -> egui::CursorIcon {
    if let Some(sel) = selection {
        if let Some(h) = hit_handle(sel, pos).or(match drag {
            Some(Drag::Resize(h)) => Some(h),
            _ => None,
        }) {
            return match h {
                Handle::N | Handle::S => egui::CursorIcon::ResizeVertical,
                Handle::E | Handle::W => egui::CursorIcon::ResizeHorizontal,
                Handle::Ne | Handle::Sw => egui::CursorIcon::ResizeNeSw,
                Handle::Nw | Handle::Se => egui::CursorIcon::ResizeNwSe,
            };
        }
        if sel.contains(pos) {
            return egui::CursorIcon::Move;
        }
    }
    egui::CursorIcon::Crosshair
}

fn full_uv() -> Rect {
    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
}
