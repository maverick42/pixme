//! Icon rendering, adapted from the visual reference (MIT OR Apache-2.0; see
//! ATTRIBUTION.md). Lucide SVGs are embedded (see `icon_data.rs`), restyled
//! to white strokes at 1.75 px, rasterized at the exact on-screen pixel size
//! by egui's SVG image loader (crisp at any DPI), and tinted per use.

use std::collections::HashMap;
use std::sync::OnceLock;

use egui::{Color32, ImageSource, Rect, Ui, Vec2, load::SizedTexture};

use super::icon_data;

/// Every icon PixMe bundles (see `icon_data.rs` for the byte table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// Capture the full screen.
    Monitor,
    /// Capture a rectangular region.
    SquareDashed,
    /// Capture the active (frontmost) window.
    AppWindow,
    /// Capture a window or an object picked under the cursor.
    SquareDashedMousePointer,
    /// Capture a fixed-size (resizable) region.
    Crop,
    /// Pin a capture to the screen as a floating window.
    Pin,
    /// Delay before capture.
    Clock,
    /// File menu (capture viewer's documents).
    Folder,
    /// Capture options menu.
    Settings,
    /// Quit the app / close.
    X,
    /// Dropdown affordance.
    ChevronDown,
    /// Checkmark (selected menu item).
    Check,
    /// Add a tab / start a new capture (viewer's "+").
    Plus,
    /// Viewfinder frame (menu-bar status item glyph).
    Scan,
    /// Drag handle on the left edge of the floating bar.
    GripVertical,
    /// Copy to clipboard (overlapping rectangles).
    Copy,
    /// Save to file (download arrow).
    Download,
}

/// One styled icon ready for egui's image loaders: white-stroked SVG bytes
/// behind a `bytes://` URI.
struct StyledIcon {
    uri: &'static str,
    bytes: egui::load::Bytes,
}

fn styled_icons() -> &'static HashMap<Icon, StyledIcon> {
    static MAP: OnceLock<HashMap<Icon, StyledIcon>> = OnceLock::new();
    MAP.get_or_init(|| {
        icon_data::DATA
            .iter()
            .map(|&(icon, name, raw)| {
                // Lucide strokes `currentColor`; we rasterize white once, then
                // tint per widget state. 1.75 keeps strokes crisp at 16 px.
                let mut svg = String::from_utf8_lossy(raw).into_owned();
                svg = svg.replace("currentColor", "#ffffff");
                svg = svg.replace("stroke-width=\"2\"", "stroke-width=\"1.75\"");
                (
                    icon,
                    StyledIcon {
                        uri: Box::leak(format!("bytes://icons/{name}.svg").into_boxed_str()),
                        bytes: egui::load::Bytes::from(svg.into_bytes()),
                    },
                )
            })
            .collect()
    })
}

/// Whether `icon` has embedded bytes (always true; used by tests).
pub fn exists(icon: Icon) -> bool {
    styled_icons().contains_key(&icon)
}

/// An egui image for `icon`, tinted. Relies on
/// `egui_extras::install_image_loaders` (done in `ui::install`).
pub fn image(icon: Icon, size: f32, tint: Color32) -> egui::Image<'static> {
    let styled = &styled_icons()[&icon];
    egui::Image::new(ImageSource::Bytes {
        uri: styled.uri.into(),
        bytes: styled.bytes.clone(),
    })
    .fit_to_exact_size(Vec2::splat(size))
    .tint(tint)
}

/// Paints `icon` centred in `rect`.
pub fn paint(ui: &Ui, rect: Rect, icon: Icon, size: f32, tint: Color32) {
    let r = Rect::from_center_size(rect.center(), Vec2::splat(size));
    image(icon, size, tint).paint_at(ui, r);
}

/// Rasterizes `icon` to raw RGBA8 bytes at `size` px — used for OS surfaces
/// that need pixel data instead of an egui image (macOS status item icon,
/// white strokes work fine for a template icon).
pub fn rgba(icon: Icon, size: u32) -> Vec<u8> {
    rasterize(icon, size).as_raw().to_vec()
}

/// Rasterizes `icon` recoloured to `color` (a `#rrggbb` hex string) — for OS
/// surfaces without a template mode (the Windows tray, where pure-white
/// strokes would vanish on a light taskbar).
pub fn rgba_with(icon: Icon, size: u32, color: &str) -> Vec<u8> {
    let Some(&(_, _, raw)) = icon_data::DATA.iter().find(|&&(i, _, _)| i == icon) else {
        return Vec::new();
    };
    let svg = String::from_utf8_lossy(raw).replace("currentColor", color);
    egui_extras::image::load_svg_bytes_with_size(
        svg.as_bytes(),
        egui::load::SizeHint::Size {
            width: size,
            height: size,
            maintain_aspect_ratio: true,
        },
        &Default::default(),
    )
    .map(|img| img.as_raw().to_vec())
    .unwrap_or_default()
}

/// Rasterizes `icon` at `size` px — used by tests, offscreen rendering and
/// OS surfaces that need pixel data (status item icon).
fn rasterize(icon: Icon, size: u32) -> egui::ColorImage {
    let styled = &styled_icons()[&icon];
    egui_extras::image::load_svg_bytes_with_size(
        styled.bytes.as_ref(),
        egui::load::SizeHint::Size {
            width: size,
            height: size,
            maintain_aspect_ratio: true,
        },
        &Default::default(),
    )
    // A bundled, compile-time-verified SVG cannot fail; fall back to an
    // empty image rather than panic if the loader ever disagrees.
    .unwrap_or_else(|_| egui::ColorImage::filled([size as usize; 2], Color32::TRANSPARENT))
}

/// Texture handle for `icon` at `size` px (public API for custom painting).
pub fn texture(ctx: &egui::Context, icon: Icon, size: u32) -> SizedTexture {
    let styled = &styled_icons()[&icon];
    let handle = ctx.load_texture(
        styled.uri,
        egui_extras::image::load_svg_bytes_with_size(
            styled.bytes.as_ref(),
            egui::load::SizeHint::Size {
                width: size,
                height: size,
                maintain_aspect_ratio: true,
            },
            &Default::default(),
        )
        .unwrap_or_else(|_| egui::ColorImage::filled([size as usize; 2], Color32::TRANSPARENT)),
        egui::TextureOptions::LINEAR,
    );
    SizedTexture::from_handle(&handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_rasterizes() {
        for &(icon, name, _) in icon_data::DATA {
            assert!(exists(icon), "{name} missing from map");
            let img = rasterize(icon, 16);
            assert_eq!(img.size, [16, 16], "{name} wrong size");
            assert!(
                img.pixels.iter().any(|p| p.a() > 0),
                "{name} rasterized fully transparent"
            );
        }
    }
}
