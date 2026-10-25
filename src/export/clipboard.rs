//! Copy-to-clipboard export via `arboard`.

use super::ExportError;
use crate::capture::Capture;
use crate::editor::Document;

/// Puts `capture`'s raw RGBA pixels on the OS clipboard — used by pinned
/// windows and by the quick-copy option.
pub fn copy_capture(capture: &Capture) -> Result<(), ExportError> {
    copy_image(&capture.image)
}

/// Puts raw RGBA pixels on the OS clipboard — shared by captures, viewer
/// documents and (cropped) selections.
pub fn copy_image(img: &image::RgbaImage) -> Result<(), ExportError> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| ExportError::Failed(e.to_string()))?;
    clipboard
        .set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: std::borrow::Cow::Borrowed(img.as_raw().as_slice()),
        })
        .map_err(|e| ExportError::Failed(e.to_string()))
}

/// Reads an image from the OS clipboard (the viewer's Paste command).
/// Returns `None` when the clipboard holds no image.
pub fn clipboard_image() -> Result<Option<image::RgbaImage>, ExportError> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| ExportError::Failed(e.to_string()))?;
    match clipboard.get_image() {
        Ok(data) => Ok(image::RgbaImage::from_raw(
            data.width as u32,
            data.height as u32,
            data.bytes.into_owned(),
        )),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(e) => Err(ExportError::Failed(e.to_string())),
    }
}

/// Renders `doc` and puts the RGBA pixels on the OS clipboard. Phase 4.
pub fn copy(_doc: &Document) -> Result<(), ExportError> {
    Err(ExportError::Failed("not implemented".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Touches the real OS clipboard — restores it afterwards.
    #[test]
    #[ignore = "requires a live OS clipboard"]
    fn image_round_trip_preserves_alpha() {
        let backup = clipboard_image().ok().flatten();
        let mut img = image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 20, 30, 255]));
        img.put_pixel(0, 0, image::Rgba([1, 2, 3, 0]));
        copy_image(&img).expect("set");
        let back = clipboard_image().expect("get").expect("image on clipboard");
        assert_eq!(
            back.get_pixel(0, 0)[3],
            0,
            "alpha lost in the clipboard round-trip"
        );
        assert_eq!(back.get_pixel(4, 4)[3], 255);
        if let Some(img) = backup {
            let _ = copy_image(&img);
        }
    }
}
