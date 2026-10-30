//! A colour with opacity, held as four bytes and written `#RRGGBBAA` (the cursor's click colour). It is `Copy`, so the
//! style the engine resolves for every frame holds nothing on the heap; the JSON and the TS type stay a
//! string.

use std::borrow::Cow;
use std::fmt;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// `[red, green, blue, alpha]`, each 0–255.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    /// `#RRGGBBAA` in either case; None for anything else (no short or opaque-only form).
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#')?.as_bytes();
        // Hex digits only: `from_str_radix` alone would take a sign.
        if hex.len() != 8 || !hex.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let mut bytes = [0; 4];
        for (byte, pair) in bytes.iter_mut().zip(hex.chunks_exact(2)) {
            *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        Some(Self(bytes))
    }
}

impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b, a] = self.0;
        write!(f, "#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Hex;
        impl de::Visitor<'_> for Hex {
            type Value = Rgba;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a colour with opacity, like #FFFFFF80")
            }
            fn visit_str<E: de::Error>(self, text: &str) -> Result<Rgba, E> {
                Rgba::parse(text).ok_or_else(|| E::invalid_value(de::Unexpected::Str(text), &self))
            }
        }
        deserializer.deserialize_str(Hex)
    }
}

impl JsonSchema for Rgba {
    fn schema_name() -> Cow<'static, str> {
        "Rgba".into()
    }
    fn inline_schema() -> bool {
        true
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "pattern": "^#[0-9A-Fa-f]{8}$" })
    }
}
