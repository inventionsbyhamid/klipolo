//! Bytes on the render worker's reply lines: a `Blob` is base64 text of at most `MAX_BLOB_BYTES` decoded,
//! a `Key` a SHA-256 in hex. The worker fits every image to the cap (`fit_jpeg`, m6); the parent takes only a JPEG
//! within the size it asked for and encodes it again itself (`reencode_jpeg`, m10), so what the UI or the library
//! stores is never the worker's file.

use std::io::Cursor;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ImageFormat, RgbImage};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::MAX_BLOB_BYTES;

/// A thumbnail's longest edge, at most (the card's frames).
pub const THUMBNAIL_EDGE: u32 = 512;
/// A preview frame's longest edge, at most (`longEdge` 256–1024).
pub const MAX_FRAME_EDGE: u32 = 1024;
/// The quality the worker tries first, and the lowest before it makes the image smaller.
const QUALITIES: [u8; 4] = [85, 75, 67, 60];

/// Bytes, base64 on the wire: an image or a normalised scene, never a path.
#[derive(Clone, PartialEq, Eq)]
pub struct Blob(Vec<u8>);

impl Blob {
    pub fn new(bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(format!("{} bytes, over the {MAX_BLOB_BYTES}", bytes.len()));
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Blob({} bytes)", self.0.len())
    }
}

impl Serialize for Blob {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&BASE64.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Blob {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        // Measured before decoding, so an oversize blob costs no allocation.
        if text.len() > MAX_BLOB_BYTES.div_ceil(3) * 4 {
            return Err(D::Error::custom("a blob over the cap"));
        }
        let bytes = BASE64
            .decode(text.as_bytes())
            .map_err(|_| D::Error::custom("a blob that isn't base64"))?;
        Blob::new(bytes).map_err(D::Error::custom)
    }
}

/// A SHA-256 (a contribution's key), as 64 lowercase hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key(pub [u8; 32]);

impl Serialize for Key {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let hex: String = self.0.iter().map(|b| format!("{b:02x}")).collect();
        serializer.serialize_str(&hex)
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        let digit = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return Err(D::Error::custom("a key is 64 hex digits"));
        }
        let mut key = [0u8; 32];
        for (i, pair) in bytes.chunks(2).enumerate() {
            match (digit(pair[0]), digit(pair[1])) {
                (Some(hi), Some(lo)) => key[i] = hi << 4 | lo,
                _ => return Err(D::Error::custom("a key is 64 hex digits")),
            }
        }
        Ok(Key(key))
    }
}

/// `image` as a JPEG within `MAX_BLOB_BYTES`: quality from 85 down to 60, then the size down by √2 a step, so a
/// legal request never produces an oversize reply.
pub fn fit_jpeg(image: &RgbImage) -> Vec<u8> {
    let mut smaller: Option<RgbImage> = None;
    loop {
        let current = smaller.as_ref().unwrap_or(image);
        let qualities: &[u8] = if smaller.is_none() {
            &QUALITIES
        } else {
            &QUALITIES[QUALITIES.len() - 1..]
        };
        for &quality in qualities {
            let bytes = encode(current, quality);
            if bytes.len() <= MAX_BLOB_BYTES || current.width().max(current.height()) <= 16 {
                return bytes;
            }
        }
        let side = |n: u32| ((f64::from(n) / std::f64::consts::SQRT_2).round() as u32).max(1);
        let (width, height) = (side(current.width()), side(current.height()));
        smaller = Some(image::imageops::resize(
            current,
            width,
            height,
            FilterType::Triangle,
        ));
    }
}

/// The parent's own copy of an image the worker sent: refused unless it is a JPEG whose sides are both within
/// `max_edge`, then decoded (within those limits) and encoded again as `fit_jpeg` does, so it stays a blob.
pub fn reencode_jpeg(bytes: &[u8], max_edge: u32) -> Result<Vec<u8>, String> {
    if !bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Err("not a JPEG".into());
    }
    let mut reader = image::io::Reader::with_format(Cursor::new(bytes), ImageFormat::Jpeg);
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(max_edge);
    limits.max_image_height = Some(max_edge);
    limits.max_alloc = Some(u64::from(max_edge) * u64::from(max_edge) * 4 + (1 << 20));
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| format!("not a JPEG within {max_edge} px: {e}"))?;
    let bytes = fit_jpeg(&image.to_rgb8());
    if bytes.is_empty() {
        return Err("the JPEG couldn't be encoded again".into());
    }
    Ok(bytes)
}

/// A baseline JPEG at `quality`; empty if the encoder refused (it doesn't for RGB).
fn encode(image: &RgbImage, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    match JpegEncoder::new_with_quality(&mut out, quality).encode_image(image) {
        Ok(()) => out,
        Err(_) => Vec::new(),
    }
}
