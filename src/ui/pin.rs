//! Pinned captures: autonomous, always-on-top floating windows that keep
//! a capture on screen — the functional reference's pin, drawn with the
//! app's chrome. A pin owns its image, opens where its source region was
//! captured, and dies with the app (closing the root closes every child
//! viewport).

use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2};

use crate::app::{Command, Shared};
use crate::capture::Capture;

use super::{
    icons::{self, Icon},
    theme::{self, Tokens},
};

/// macOS traffic-light red for the hover close dot.
const CLOSE_RED: Color32 = Color32::from_rgb(0xFF, 0x5F, 0x57);
/// Bottom control strip metrics.
const STRIP_H: f32 = 26.0;
const STRIP_PAD: f32 = 10.0;
/// Zoom presets of the percent menu.
const ZOOMS: [f32; 3] = [0.5, 1.0, 2.0];
/// Corner rounding of the pinned image.
const ROUNDING: f32 = 10.0;
/// Dark scoop hugging the top-left corner behind the close dot.
const HALO: f32 = 34.0;
/// Corner-resize hit zones (squares at each window corner).
const GRIP: f32 = 20.0;
/// Below this width/height only the close dot is shown — there is no
/// room left for the strip icons.
const MIN_STRIP_W: f32 = 210.0;
const MIN_STRIP_H: f32 = 90.0;
/// Below this width the zoom percent is hidden first (the icons matter
/// more).
const MIN_ZOOM_W: f32 = 280.0;
/// The centre grip pill needs more room than the round buttons.
const MIN_PILL_W: f32 = 300.0;
/// Smallest on-screen side a pin can be resized to (logical px).
pub const MIN_SIDE: f32 = 64.0;
/// Largest zoom (logical units per image point).
pub const MAX_ZOOM: f32 = 6.0;

/// Which window corner is being dragged while resizing: it follows the
/// pointer, the opposite corner stays put.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PinAnchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl PinAnchor {
    /// Sign applied to the pointer delta per axis: for `TopLeft`, moving
    /// the pointer left/up grows the window.
    fn signs(self) -> (f32, f32) {
        match self {
            Self::TopLeft => (-1.0, -1.0),
            Self::TopRight => (1.0, -1.0),
            Self::BottomLeft => (-1.0, 1.0),
            Self::BottomRight => (1.0, 1.0),
        }
    }

    /// The resize cursor shown on that corner.
    fn cursor(self) -> egui::CursorIcon {
        match self {
            Self::TopLeft | Self::BottomRight => egui::CursorIcon::ResizeNwSe,
            Self::TopRight | Self::BottomLeft => egui::CursorIcon::ResizeNeSw,
        }
    }

    /// The corner that stays put while this one is dragged.
    fn opposite(self) -> Self {
        match self {
            Self::TopLeft => Self::BottomRight,
            Self::TopRight => Self::BottomLeft,
            Self::BottomLeft => Self::TopRight,
            Self::BottomRight => Self::TopLeft,
        }
    }
}

/// The resize hit zone at `anchor`'s corner of `rect`.
fn corner_zone(anchor: PinAnchor, rect: Rect) -> Rect {
    match anchor {
        PinAnchor::TopLeft => Rect::from_min_size(rect.min, Vec2::splat(GRIP)),
        PinAnchor::TopRight => {
            Rect::from_min_size(Pos2::new(rect.max.x - GRIP, rect.min.y), Vec2::splat(GRIP))
        }
        PinAnchor::BottomLeft => {
            Rect::from_min_size(Pos2::new(rect.min.x, rect.max.y - GRIP), Vec2::splat(GRIP))
        }
        PinAnchor::BottomRight => {
            Rect::from_min_size(rect.max - Vec2::splat(GRIP), Vec2::splat(GRIP))
        }
    }
}

/// One pinned capture — an autonomous floating window.
pub struct PinWindow {
    /// Identity; the viewport id is `("pixme.pin", id)` hashed.
    pub id: u32,
    /// The pinned image plus its monitor info (shared, cheap to clone).
    pub capture: Arc<Capture>,
    /// Global logical position the pin opened at — where its source
    /// region sat on screen.
    pub origin: Pos2,
    /// Display zoom (logical units per image point at scale 1). Updated
    /// by the % menu and by the corner-resize grips.
    pub zoom: f32,
    /// GPU texture, built once.
    texture: Option<egui::TextureHandle>,
    /// (zoom, size, press origin, corner) latched when a resize begins —
    /// the delta is measured from the press point while the button stays
    /// down.
    resize_start: Option<(f32, Vec2, Pos2, PinAnchor)>,
}

impl PinWindow {
    pub fn new(id: u32, capture: Capture, origin: Pos2) -> Self {
        Self {
            id,
            capture: Arc::new(capture),
            origin,
            zoom: 1.0,
            texture: None,
            resize_start: None,
        }
    }

    /// Resize the window to `zoom`, keeping `anchor`'s corner fixed —
    /// sent to THIS viewport directly (the pin is autonomous; no command
    /// round-trip through the shared queue).
    fn apply_zoom(&mut self, ctx: &egui::Context, z: f32, anchor: PinAnchor) {
        let nat = self.logical_size() / self.zoom;
        let z_min = (MIN_SIDE / nat.x).max(MIN_SIDE / nat.y);
        self.zoom = z.clamp(z_min, MAX_ZOOM);
        let size = self.logical_size();
        if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
            let pos = match anchor {
                PinAnchor::TopLeft => outer.min,
                PinAnchor::TopRight => Pos2::new(outer.max.x - size.x, outer.min.y),
                PinAnchor::BottomLeft => Pos2::new(outer.min.x, outer.max.y - size.y),
                PinAnchor::BottomRight => Pos2::new(outer.max.x - size.x, outer.max.y - size.y),
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }

    /// On-screen logical size at the current zoom (physical px ÷ scale).
    pub fn logical_size(&self) -> Vec2 {
        let s = self.capture.scale_factor.max(f32::EPSILON);
        Vec2::new(
            self.capture.image.width() as f32 / s,
            self.capture.image.height() as f32 / s,
        ) * self.zoom
    }

    /// The pinned image plus, on hover, its control chrome.
    pub fn show(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let ctx = ui.ctx().clone();
        let rect = ui.max_rect();
        let t = theme::tokens(&ctx);

        let img = &self.capture.image;
        let tex = self.texture.get_or_insert_with(|| {
            ctx.load_texture(
                format!("pin-{}", self.id),
                egui::ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    img.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            )
        });
        egui::Image::new(egui::load::SizedTexture::from_handle(tex))
            .corner_radius(ROUNDING)
            .paint_at(ui, rect);
        ui.painter().rect_stroke(
            rect,
            ROUNDING as u8,
            Stroke::new(1.0, t.card_border),
            egui::StrokeKind::Inside,
        );

        // The image body moves the window.
        let body = ui.interact(rect, ui.id().with("pin-body"), Sense::drag());
        if body.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        self.resize_grips(ui, &ctx, rect);

        let hovered = ctx
            .input(|i| i.pointer.latest_pos())
            .is_some_and(|p| rect.contains(p));
        if hovered {
            self.chrome(ui, &ctx, rect, &t, shared);
        }
    }

    /// Aspect-locked resize at all four corners: the dragged corner
    /// follows the pointer, the opposite one stays put. A press *on the
    /// grip widget* latches the resize (`is_pointer_button_down_on`, no
    /// drag threshold needed) — presses claimed by higher widgets (the
    /// close dot sits inside the TL zone) never start one.
    fn resize_grips(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, rect: Rect) {
        let pointer_down = ctx.input(|i| i.pointer.primary_down());
        let press_origin = ctx.input(|i| i.pointer.press_origin());
        let pointer_pos = ctx.input(|i| i.pointer.latest_pos());
        let anchors = [
            PinAnchor::TopLeft,
            PinAnchor::TopRight,
            PinAnchor::BottomLeft,
            PinAnchor::BottomRight,
        ];

        for anchor in anchors {
            let r = ui.interact(
                corner_zone(anchor, rect),
                ui.id().with(("pin-resize", anchor as u8)),
                Sense::drag(),
            );
            if r.is_pointer_button_down_on()
                && self.resize_start.is_none()
                && let Some(origin) = press_origin
            {
                self.resize_start = Some((self.zoom, self.logical_size(), origin, anchor));
            }
        }

        // While the button stays down the latched corner follows the
        // pointer; release clears the latch.
        let mut live_anchor = None;
        if pointer_down {
            if let Some((z0, s0, origin, anchor)) = self.resize_start
                && let Some(now) = pointer_pos
            {
                let (sx, sy) = anchor.signs();
                let d = now - origin;
                // Grow along whichever axis the drag extends the most —
                // the aspect ratio is locked to the image, and the
                // OPPOSITE corner stays bolted in place.
                let kx = (s0.x + sx * d.x) / s0.x;
                let ky = (s0.y + sy * d.y) / s0.y;
                self.apply_zoom(ctx, z0 * kx.max(ky), anchor.opposite());
                live_anchor = Some(anchor);
            }
        } else {
            self.resize_start = None;
        }

        for anchor in anchors {
            let zone = corner_zone(anchor, rect);
            if zone.contains(pointer_pos.unwrap_or_default()) || live_anchor == Some(anchor) {
                ctx.set_cursor_icon(anchor.cursor());
            }
        }
    }

    /// Hover controls: halo + red close dot top-left; bottom strip with
    /// the zoom percent, a drag grip, and copy / save buttons — hidden
    /// progressively when the window shrinks.
    fn chrome(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        rect: Rect,
        t: &Tokens,
        shared: &Shared,
    ) {
        // -- dark scoop hugging the corner behind the close dot -----------
        let halo = Rect::from_min_size(rect.min, Vec2::splat(HALO));
        ui.painter().rect_filled(
            halo,
            CornerRadius {
                nw: ROUNDING as u8,
                ne: 0,
                sw: 0,
                se: 16,
            },
            t.dock,
        );

        // -- red close dot -------------------------------------------------
        let c = rect.min + Vec2::splat(16.0);
        let dot = Rect::from_center_size(c, Vec2::splat(18.0));
        let dot_resp = ui.interact(dot, ui.id().with("pin-close"), Sense::click());
        let fill = if dot_resp.hovered() {
            t.danger
        } else {
            CLOSE_RED
        };
        ui.painter().circle_filled(c, 7.0, fill);
        if dot_resp.hovered() {
            icons::paint(
                ui,
                dot.shrink(5.0),
                Icon::X,
                9.0,
                Color32::from_black_alpha(150),
            );
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if dot_resp.clicked() {
            shared.push(Command::ClosePin(self.id));
            return;
        }

        // Progressive chrome: on a tiny pin the close dot is all that fits.
        if rect.width() < MIN_STRIP_W || rect.height() < MIN_STRIP_H {
            return;
        }
        let strip_y = rect.max.y - STRIP_PAD - STRIP_H;

        // -- zoom percent pill (bottom-left) ------------------------------
        if rect.width() >= MIN_ZOOM_W {
            let zoom_rect = Rect::from_min_size(
                Pos2::new(rect.min.x + STRIP_PAD, strip_y),
                Vec2::new(56.0, STRIP_H),
            );
            let zoom_resp = ui.interact(zoom_rect, ui.id().with("pin-zoom"), Sense::click());
            ui.painter().rect_filled(
                zoom_rect,
                STRIP_H / 2.0,
                if zoom_resp.hovered() { t.hover } else { t.card },
            );
            ui.painter().rect_stroke(
                zoom_rect,
                STRIP_H / 2.0,
                Stroke::new(1.0, t.card_border),
                egui::StrokeKind::Outside,
            );
            ui.painter().text(
                zoom_rect.center(),
                Align2::CENTER_CENTER,
                format!("{}%", (self.zoom * 100.0).round() as u32),
                theme::semibold(11.0),
                t.text,
            );
            egui::Popup::menu(&zoom_resp).show(|ui| {
                for z in ZOOMS {
                    let label = format!("{}%", (z * 100.0).round() as u32);
                    if ui
                        .selectable_label((self.zoom - z).abs() < f32::EPSILON, label)
                        .clicked()
                    {
                        self.apply_zoom(ctx, z, PinAnchor::TopLeft);
                        ui.close();
                    }
                }
            });
        }

        // -- centre pill: drag grip (also moves the window) ---------------
        if rect.width() >= MIN_PILL_W {
            let pill = Rect::from_center_size(
                Pos2::new(rect.center().x, strip_y + STRIP_H / 2.0),
                Vec2::new(72.0, STRIP_H),
            );
            let grip = ui.interact(pill, ui.id().with("pin-grip"), Sense::drag());
            ui.painter().rect_filled(
                pill,
                STRIP_H / 2.0,
                if grip.hovered() {
                    theme::pin_chrome::BG_HOVER
                } else {
                    theme::pin_chrome::BG
                },
            );
            ui.painter().rect_stroke(
                pill,
                STRIP_H / 2.0,
                Stroke::new(1.0, theme::pin_chrome::BORDER),
                egui::StrokeKind::Outside,
            );
            icons::paint(
                ui,
                Rect::from_center_size(
                    Pos2::new(pill.center().x - 12.0, pill.center().y),
                    Vec2::splat(16.0),
                ),
                Icon::GripVertical,
                14.0,
                theme::pin_chrome::ICON,
            );
            icons::paint(
                ui,
                Rect::from_center_size(
                    Pos2::new(pill.center().x + 12.0, pill.center().y),
                    Vec2::splat(16.0),
                ),
                Icon::Pin,
                13.0,
                theme::pin_chrome::ICON,
            );
            if grip.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            grip.on_hover_cursor(egui::CursorIcon::Grab);
        }

        // -- right: copy and save round buttons ---------------------------
        for (n, icon, cmd) in [
            (0, Icon::Copy, Command::CopyPin(self.id)),
            (1, Icon::Download, Command::SavePin(self.id)),
        ] {
            let c = Pos2::new(
                rect.max.x - STRIP_PAD - STRIP_H / 2.0 - n as f32 * (STRIP_H + 8.0),
                strip_y + STRIP_H / 2.0,
            );
            let cr = Rect::from_center_size(c, Vec2::splat(STRIP_H));
            let r = ui.interact(cr, ui.id().with(("pin-act", n)), Sense::click());
            ui.painter().circle_filled(
                c,
                STRIP_H / 2.0,
                if r.hovered() {
                    theme::pin_chrome::BG_HOVER
                } else {
                    theme::pin_chrome::BG
                },
            );
            ui.painter().circle_stroke(
                c,
                STRIP_H / 2.0,
                Stroke::new(1.0, theme::pin_chrome::BORDER),
            );
            icons::paint(ui, cr.shrink(7.0), icon, 13.0, theme::pin_chrome::ICON);
            let tip = match n {
                0 => t!("pin.copy"),
                _ => t!("pin.save"),
            };
            let clicked = r.clicked();
            r.on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(tip);
            if clicked {
                shared.push(cmd);
            }
        }
    }
}
