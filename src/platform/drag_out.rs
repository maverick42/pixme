//! OS drag-out of a capture into external applications.
//!
//! All `drag` crate usage is confined to this module so it can be replaced in
//! one place if ever needed. Both OSes receive an absolute PNG file path on
//! disk (`DragItem::Files`): `DragItem::Data` is macOS-only in `drag` 2.1.1,
//! and a real file is what Finder / Explorer, browsers, Slack, Discord and
//! mail clients all accept uniformly.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use image::RgbaImage;
use raw_window_handle::{HasWindowHandle, RawWindowHandle, WindowHandle};

/// Subdirectory inside [`std::env::temp_dir`] holding dragged PNG files.
const DRAG_DIR: &str = "pixme-drag";

/// Maximum age of a temporary drag file before [`cleanup_stale_files`] removes
/// it on startup. Files are NOT deleted in the drop callback: the receiving
/// application reads the file asynchronously after the drop returns.
const STALE_AFTER: Duration = Duration::from_secs(3600);

/// Longest side (in pixels) of the cursor-following drag preview image.
/// Passing the full-resolution capture (e.g. 5K) as the drag icon produces a
/// giant preview that covers the whole screen (`drag-rs` issue #41), while the
/// dropped file itself must always be the full-resolution rendered capture.
const PREVIEW_MAX_SIDE: u32 = 160;

/// Deletes files in `temp_dir()/pixme-drag/` older than one hour. Best-effort:
/// any I/O failure is ignored so startup is never blocked.
pub fn cleanup_stale_files() {
    cleanup_dir_older_than(&drag_temp_dir(), STALE_AFTER, SystemTime::now());
}

fn drag_temp_dir() -> PathBuf {
    std::env::temp_dir().join(DRAG_DIR)
}

fn cleanup_dir_older_than(dir: &Path, max_age: Duration, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let expired = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= max_age);
        if expired {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Turns a tab title into a safe file stem for macOS and Windows: strips path
/// separators, Windows-forbidden characters (`<>:"/\|?*`), control characters
/// and trailing dots/spaces, and avoids Windows reserved device names. Falls
/// back to `"capture"` when nothing usable remains.
pub fn sanitize_file_stem(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']);
    // Strip a trailing image extension if the tab title already has one (e.g.
    // an opened file tab named `photo.png`).
    let stem = Path::new(trimmed)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|e| {
            Path::new(trimmed)
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| {
                    matches!(
                        x.to_ascii_lowercase().as_str(),
                        "png" | "jpg" | "jpeg" | "webp"
                    )
                })
                && !e.is_empty()
        })
        .unwrap_or(trimmed)
        .trim()
        .trim_end_matches(['.', ' ']);

    let candidate: String = stem.chars().take(120).collect();
    let candidate = candidate.trim().trim_end_matches(['.', ' ']);
    if candidate.is_empty() || is_windows_reserved(candidate) {
        "capture".to_string()
    } else {
        candidate.to_string()
    }
}

fn is_windows_reserved(stem: &str) -> bool {
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// Writes `full_image` (the full-resolution rendered document) as a PNG in
/// `dir/<sanitized-title>.png` via an atomic temp-file rename so a concurrent
/// reader never observes a half-written file. Returns the absolute path.
fn write_drag_png(dir: &Path, title: &str, full_image: &RgbaImage) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stem = sanitize_file_stem(title);
    let final_path = dir.join(format!("{stem}.png"));
    let tmp_path = dir.join(format!(".{stem}.{}.tmp", std::process::id()));
    let bytes = crate::store::ser::png_encode(full_image)?;
    std::fs::write(&tmp_path, &bytes)?;
    std::fs::rename(&tmp_path, &final_path)?;
    // Ensure the path handed to the OS is canonical / absolute without UNC
    // `\\?\` prefixes on Windows (which many drop targets reject).
    Ok(if final_path.is_absolute() {
        final_path
    } else {
        std::env::current_dir()?.join(final_path)
    })
}

/// Builds the small PNG byte buffer used as the cursor-following drag preview.
/// Downscales `full_image` so its longest side is at most [`PREVIEW_MAX_SIDE`].
fn encode_preview_png(full_image: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    let (w, h) = full_image.dimensions();
    let longest = w.max(h);
    if longest <= PREVIEW_MAX_SIDE || longest == 0 {
        return crate::store::ser::png_encode(full_image);
    }
    let scale = PREVIEW_MAX_SIDE as f32 / longest as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    let small = image::imageops::resize(full_image, nw, nh, image::imageops::FilterType::Triangle);
    crate::store::ser::png_encode(&small)
}

/// Starts an OS drag-and-drop session carrying `full_image` (the full
/// rendered capture, base + annotations) as a PNG file named after `title`.
///
/// Must be called on the main / UI thread while the primary mouse button is
/// held down. Both OSes run their own tracking loop (`beginDraggingSession…`
/// on macOS, `DoDragDrop` on Windows) and swallow the mouse-up event; callers
/// must release egui's pointer state afterwards so no stuck drag remains.
pub fn start_image_drag(title: &str, full_image: &RgbaImage) -> Result<(), String> {
    let Some(window) = ActiveOsWindow::current() else {
        return Err("no active OS window for drag-out".into());
    };
    let path = write_drag_png(&drag_temp_dir(), title, full_image)
        .map_err(|e| format!("could not write drag PNG: {e}"))?;
    let preview = encode_preview_png(full_image)
        .map_err(|e| format!("could not encode drag preview: {e}"))?;

    drag::start_drag(
        &window,
        drag::DragItem::Files(vec![path]),
        drag::Image::Raw(preview),
        |_result, _pos| {},
        drag::Options::default(),
    )
    .map_err(|e| format!("OS drag-out failed: {e}"))
}

/// Minimal [`HasWindowHandle`] wrapper around the currently active OS window
/// (the viewer or pinned viewport is in front because the user just pressed
/// and dragged inside it). Necessary because egui deferred viewports do not
/// receive an `eframe::Frame`.
struct ActiveOsWindow(RawWindowHandle);

impl HasWindowHandle for ActiveOsWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: the handle is queried on the main thread from the live window
        // receiving the current mouse drag and used synchronously within
        // `start_image_drag`.
        unsafe { Ok(WindowHandle::borrow_raw(self.0)) }
    }
}

impl ActiveOsWindow {
    #[cfg(target_os = "macos")]
    fn current() -> Option<Self> {
        use raw_window_handle::AppKitWindowHandle;
        use std::ffi::c_void;
        use std::ptr::NonNull;

        #[link(name = "objc", kind = "dylib")]
        unsafe extern "C" {
            fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
            fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
            fn objc_msgSend();
        }

        type MsgSendId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;

        unsafe {
            let send_id: MsgSendId = std::mem::transmute(objc_msgSend as *const ());
            let app_cls = objc_getClass(c"NSApplication".as_ptr());
            if app_cls.is_null() {
                return None;
            }
            let app = send_id(app_cls, sel_registerName(c"sharedApplication".as_ptr()));
            if app.is_null() {
                return None;
            }
            let mut win = send_id(app, sel_registerName(c"keyWindow".as_ptr()));
            if win.is_null() {
                win = send_id(app, sel_registerName(c"mainWindow".as_ptr()));
            }
            if win.is_null() {
                return None;
            }
            let view = send_id(win, sel_registerName(c"contentView".as_ptr()));
            let ns_view = NonNull::new(view)?;
            Some(Self(RawWindowHandle::AppKit(AppKitWindowHandle::new(
                ns_view,
            ))))
        }
    }

    #[cfg(target_os = "windows")]
    fn current() -> Option<Self> {
        use raw_window_handle::Win32WindowHandle;
        use std::num::NonZeroIsize;

        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetActiveWindow() -> isize;
            fn GetForegroundWindow() -> isize;
        }

        unsafe {
            let mut hwnd = GetActiveWindow();
            if hwnd == 0 {
                hwnd = GetForegroundWindow();
            }
            let nz = NonZeroIsize::new(hwnd)?;
            Some(Self(RawWindowHandle::Win32(Win32WindowHandle::new(nz))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_illegal_chars_and_extensions() {
        assert_eq!(sanitize_file_stem("Capture #1"), "Capture #1");
        assert_eq!(
            sanitize_file_stem("a/b\\c:d*e?f\"g<h>i|j"),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_file_stem("photo.png"), "photo");
        assert_eq!(sanitize_file_stem("shot.JPG"), "shot");
        assert_eq!(sanitize_file_stem("   ...   "), "capture");
        assert_eq!(sanitize_file_stem(""), "capture");
        assert_eq!(sanitize_file_stem("CON"), "capture");
        assert_eq!(sanitize_file_stem("nul.png"), "capture");
        assert_eq!(sanitize_file_stem("trailing. "), "trailing");
    }

    #[test]
    fn write_drag_png_writes_full_resolution_file() {
        let dir = std::env::temp_dir().join(format!("pixme-drag-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let img = RgbaImage::from_pixel(320, 200, image::Rgba([12, 34, 56, 255]));
        let path = write_drag_png(&dir, "My Capture: 01", &img).expect("write drag png");
        assert!(path.is_absolute());
        assert_eq!(
            path.file_name().and_then(|s| s.to_str()),
            Some("My Capture_ 01.png")
        );
        let decoded =
            crate::store::ser::png_decode(&std::fs::read(&path).expect("read")).expect("decode");
        assert_eq!(decoded.dimensions(), (320, 200));
        assert_eq!(*decoded.get_pixel(0, 0), image::Rgba([12, 34, 56, 255]));

        // Stale cleanup removes old files and keeps fresh ones.
        let future = SystemTime::now() + STALE_AFTER + Duration::from_secs(10);
        cleanup_dir_older_than(&dir, STALE_AFTER, future);
        assert!(!path.exists(), "expired drag file should be cleaned up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_downscales_large_images_only() {
        let big = RgbaImage::from_pixel(1000, 500, image::Rgba([1, 2, 3, 255]));
        let bytes = encode_preview_png(&big).expect("preview");
        let decoded = crate::store::ser::png_decode(&bytes).expect("decode");
        assert_eq!(decoded.dimensions(), (160, 80));

        let small = RgbaImage::from_pixel(80, 40, image::Rgba([1, 2, 3, 255]));
        let bytes = encode_preview_png(&small).expect("preview");
        let decoded = crate::store::ser::png_decode(&bytes).expect("decode");
        assert_eq!(decoded.dimensions(), (80, 40));
    }
}
