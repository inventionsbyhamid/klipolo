//! Image generation: `POST /images/generations` with Muse Image, PNGs back as base64.
//!
//! Muse Image can't make a transparent PNG (its background is always opaque), and it may look things up or run code
//! for a picture unless told not to: every request turns those tools off, so a picture costs one image and nothing
//! more.

use std::fmt;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ledger::Row;
use crate::{pricing, MetaClient, MetaError};

/// Meta's image model.
pub const IMAGE_MODEL: &str = "muse-image-1.0";

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The picture's shape. Meta keeps its own pixel size (1600x1600 for a square); the size only sets the aspect ratio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageSize {
    /// `1024x1024`.
    Square,
    /// `1792x1024`.
    Landscape,
    /// `1024x1792`.
    Portrait,
}

impl ImageSize {
    /// Meta's `size` for it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Square => "1024x1024",
            Self::Landscape => "1792x1024",
            Self::Portrait => "1024x1792",
        }
    }
}

/// How hard Muse Image works on a picture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageEffort {
    /// Meta's default (`reasoning_strength` left out, which is `high`): it plans and refines the picture.
    #[default]
    Full,
    /// `reasoning_strength: low`: one generation pass, no self-refinement, sooner.
    Low,
}

/// One picture: PNG bytes, and the prompt Meta actually drew from when it said.
#[derive(Clone, PartialEq, Eq)]
pub struct GeneratedImage {
    pub png: Vec<u8>,
    pub revised_prompt: Option<String>,
}

/// The size, not the bytes.
impl fmt::Debug for GeneratedImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeneratedImage")
            .field("png", &format_args!("{} bytes", self.png.len()))
            .field("revised_prompt", &self.revised_prompt)
            .finish()
    }
}

#[derive(Deserialize)]
struct ImagesWire {
    #[serde(default)]
    data: Vec<ImageWire>,
}

#[derive(Deserialize)]
struct ImageWire {
    #[serde(default)]
    b64_json: Option<String>,
    #[serde(default)]
    revised_prompt: Option<String>,
}

impl MetaClient {
    /// `n` PNGs of `prompt` in the shape `size`, with Meta's full effort. An answer with no pictures is empty, not an
    /// error; a prompt Meta won't draw is [`MetaError::Refused`].
    pub async fn generate_image(
        &self,
        prompt: &str,
        size: ImageSize,
        n: u8,
    ) -> Result<Vec<GeneratedImage>, MetaError> {
        self.generate_image_with(prompt, size, n, ImageEffort::Full)
            .await
    }

    /// [`Self::generate_image`] with the effort given.
    pub async fn generate_image_with(
        &self,
        prompt: &str,
        size: ImageSize,
        n: u8,
        effort: ImageEffort,
    ) -> Result<Vec<GeneratedImage>, MetaError> {
        if n == 0 {
            return Err(MetaError::BadRequest {
                message: "Ask for at least one image.".into(),
            });
        }
        let body = image_request(prompt, size, n, effort);
        let request = self
            .http
            .post(self.meta_url("images/generations"))
            .json(&body);
        let response = self.send_meta(request).await?;
        let answer: ImagesWire = self.read_json(response).await?;
        // Meta bills each image it sends, whether or not it can be read here, so the row comes first.
        let returned = answer.data.len();
        if returned > 0 {
            self.record(Row {
                model: IMAGE_MODEL,
                purpose: "images/generations",
                input_tokens: None,
                output_tokens: None,
                frames_or_seconds: match returned {
                    1 => "1 image".to_owned(),
                    n => format!("{n} images"),
                },
                cost_usd: pricing::image_usd(returned),
            });
        }
        answer.data.into_iter().map(image).collect()
    }
}

/// The request: base64 PNGs, every tool off.
fn image_request(prompt: &str, size: ImageSize, n: u8, effort: ImageEffort) -> Value {
    let mut body = json!({
        "model": IMAGE_MODEL,
        "prompt": prompt,
        "n": n,
        "size": size.as_str(),
        "response_format": "b64_json",
        "output_format": "png",
        "tool_enablement": {
            "enable_image_search": false,
            "enable_web_search": false,
            "enable_shell": false,
        },
    });
    if effort == ImageEffort::Low {
        body["reasoning_strength"] = json!("low");
    }
    body
}

/// One `data` item as a PNG. A link instead of bytes, base64 that doesn't decode, or bytes that aren't a PNG are not
/// what was asked for.
fn image(wire: ImageWire) -> Result<GeneratedImage, MetaError> {
    let encoded = wire
        .b64_json
        .ok_or_else(|| MetaError::Decode("an image came without b64_json".into()))?;
    let png = STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| MetaError::Decode("an image's base64 does not decode".into()))?;
    if !png.starts_with(PNG_SIGNATURE) {
        return Err(MetaError::Decode("an image is not a PNG".into()));
    }
    let revised_prompt = wire
        .revised_prompt
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty());
    Ok(GeneratedImage {
        png,
        revised_prompt,
    })
}
