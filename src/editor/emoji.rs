//! Embedded emoji set: Noto Color Emoji PNGs (128 px, Apache 2.0) driven
//! by `assets/emoji/manifest.txt` — `build.rs` turns it into the static
//! [`EMOJI_TABLE`]. Pixels are decoded lazily, once per id, behind a
//! `OnceLock` (the thumbnail worker can race the UI thread safely).
//!
//! Ids are Unicode code points and are permanent: removing one from the
//! manifest never breaks a stored document — the renderer falls back to
//! a neutral placeholder (`draw`/`render` side), `image()` just returns
//! `None`.

use std::sync::OnceLock;

use image::RgbaImage;

/// One embedded emoji.
pub struct EmojiDef {
    /// Unicode code point — this is the id stored on `EmojiObj`.
    pub id: u32,
    /// Manifest section the entry sits under — picker grouping.
    pub section: &'static str,
    /// PNG bytes (128 px).
    png: &'static [u8],
    /// Lazily decoded RGBA — `None` when the PNG failed to decode.
    decoded: OnceLock<Option<RgbaImage>>,
}

include!(concat!(env!("OUT_DIR"), "/emoji_table.rs"));

impl EmojiDef {
    /// Decoded pixels, decoded at most once per id.
    pub fn image(&self) -> Option<&RgbaImage> {
        self.decoded
            .get_or_init(|| {
                image::load_from_memory_with_format(self.png, image::ImageFormat::Png)
                    .ok()
                    .map(|d| d.to_rgba8())
            })
            .as_ref()
    }
}

/// All emojis, in manifest order (the picker's display order).
pub fn all() -> &'static [EmojiDef] {
    &EMOJI_TABLE
}

/// Section names in first-appearance (manifest) order — the picker's
/// tab order.
pub fn sections() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for d in all() {
        if !v.contains(&d.section) {
            v.push(d.section);
        }
    }
    v
}

/// The emoji registered under `id`, if any.
pub fn find(id: u32) -> Option<&'static EmojiDef> {
    EMOJI_TABLE.iter().find(|d| d.id == id)
}

/// Decoded pixels for `id` — `None` for unknown ids AND for corrupt
/// PNGs; callers draw the neutral placeholder either way.
pub fn image(id: u32) -> Option<&'static RgbaImage> {
    find(id)?.image()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_sorted_by_documented_order_and_decodable() {
        assert!(all().len() >= 60);
        for def in all() {
            let img = def
                .image()
                .unwrap_or_else(|| panic!("emoji U+{:04X} PNG failed to decode", def.id));
            assert_eq!((img.width(), img.height()), (128, 128));
        }
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<u32> = all().iter().map(|d| d.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all().len());
    }

    #[test]
    fn unknown_id_yields_no_image_and_no_panic() {
        assert!(find(0x10FFFE).is_none());
        assert!(image(0x10FFFE).is_none());
        assert!(image(u32::from('🦄')).is_none()); // not in the curated set
    }
}
