//! serde helpers for the persisted annotation blob.
//!
//! egui types already implement `Serialize`/`Deserialize` (eframe's
//! `persistence` feature pulls `emath/serde` + `ecolor/serde`), so only
//! `RgbaImage` needs a shim: it crosses the blob as PNG-encoded bytes,
//! compact under postcard (`serde::Bytes`/`ByteBuf` use the byte path).

use image::RgbaImage;

/// PNG-encodes an RGBA image — the store's blob format for every bitmap
/// (base captures, `PicObj` images, `FillObj` masks).
pub fn png_encode(img: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    use image::ImageEncoder;
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(buf)
}

/// Decodes a PNG blob back to RGBA.
pub fn png_decode(bytes: &[u8]) -> anyhow::Result<RgbaImage> {
    Ok(image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?.to_rgba8())
}

/// `#[serde(with = "crate::store::ser::rgba_png")]` on `RgbaImage`
/// fields: serializes as PNG bytes, decodes back on load.
pub mod rgba_png {
    use serde::de::{Error as _, SeqAccess, Visitor};
    use serde::ser::Error as _;
    use serde::{Deserializer, Serializer};

    use super::RgbaImage;
    use super::{png_decode, png_encode};

    /// Serializes `img` as a PNG byte blob (postcard's compact byte path).
    pub fn serialize<S: Serializer>(img: &RgbaImage, s: S) -> Result<S::Ok, S::Error> {
        let buf = png_encode(img).map_err(S::Error::custom)?;
        s.serialize_bytes(&buf)
    }

    /// Deserializes a PNG byte blob back into RGBA pixels.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<RgbaImage, D::Error> {
        struct Bytes;
        impl<'de> Visitor<'de> for Bytes {
            type Value = Vec<u8>;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a PNG byte blob")
            }

            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                Ok(v.to_vec())
            }

            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
                Ok(v)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(b) = seq.next_element()? {
                    out.push(b);
                }
                Ok(out)
            }
        }
        let buf = d.deserialize_bytes(Bytes)?;
        png_decode(&buf).map_err(D::Error::custom)
    }
}
