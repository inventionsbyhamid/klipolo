//! Muse Image behind `ImageMaker`, through the one shared `Meta`: each request waits for one of the device's Meta slots
//! (`Meta::call`), is sent again while the relay or Traefik says busy (`retry_busy`), and all of it fits one deadline.
//! Whether the client calls Meta with the person's key or goes through the relay is the client's business. Meta's
//! types and errors stop here: each failure becomes one plain sentence.

use std::time::Duration;

use klipolo_meta::{GeneratedImage, ImageEffort, MetaError};

use super::BoxFuture;
use super::images::{ImageError, ImageErrorKind, ImageMaker, ImageSize, MadeImage};
use super::meta::Meta;
use super::retry_busy;

/// What the library calls the pictures this makes: "Made with Muse Image".
pub const MADE_WITH: &str = "Muse Image";

/// The longest a picture request may take, slot waits and busy retries included. A real sticker took 34.7 s.
pub const IMAGE_TIMEOUT: Duration = Duration::from_secs(120);

const BUSY: &str = "Muse Image is busy right now, so try again in a moment.";
const RELAY_OUT_OF_CREDIT: &str = "Klipolo's credit for Muse Image has run out, so add your own Meta key in Settings to keep going.";
const KEY_OUT_OF_CREDIT: &str =
    "Your Meta account is out of credit or has a billing problem, so check it on Meta's site.";
const REFUSED: &str = "Muse Image won't draw that, so try describing it another way.";
const TIMEOUT: &str = "Muse Image took too long to answer, so try again.";
const NOT_ALLOWED: &str = "Your Meta key or access code wasn't accepted, so check it in Settings.";
const OFFLINE: &str =
    "Klipolo couldn't reach Muse Image, so check your internet connection and try again.";
const NO_PICTURE: &str = "Muse Image didn't send a picture back, so try again.";
const FAILED: &str = "Muse Image couldn't make that picture, so try again.";

/// Muse Image through one device's shared `Meta` (clones share its slots).
#[derive(Clone, Debug)]
pub struct MetaImages {
    meta: Meta,
    timeout: Duration,
    effort: ImageEffort,
}

impl MetaImages {
    /// Muse Image at its full effort, within `IMAGE_TIMEOUT`.
    pub fn new(meta: Meta) -> Self {
        Self {
            meta,
            timeout: IMAGE_TIMEOUT,
            effort: ImageEffort::Full,
        }
    }

    /// The longest a request may take.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// One generation pass, no self-refinement: sooner, maybe rougher.
    pub fn fast(mut self) -> Self {
        self.effort = ImageEffort::Low;
        self
    }
}

impl ImageMaker for MetaImages {
    fn made_with(&self) -> &str {
        MADE_WITH
    }

    fn make<'a>(
        &'a self,
        prompt: &'a str,
        size: ImageSize,
        n: u8,
    ) -> BoxFuture<'a, Result<Vec<MadeImage>, ImageError>> {
        Box::pin(async move {
            let client = self.meta.client();
            // Only a busy moment leaves `call` as an error, for `retry_busy` to send again; any other answer comes out
            // as it is, so a final failure is never asked again on its own.
            let attempt = || {
                self.meta.call(async {
                    let size = match size {
                        ImageSize::Square => klipolo_meta::ImageSize::Square,
                        ImageSize::Landscape => klipolo_meta::ImageSize::Landscape,
                        ImageSize::Portrait => klipolo_meta::ImageSize::Portrait,
                    };
                    match client
                        .generate_image_with(prompt, size, n, self.effort)
                        .await
                    {
                        Err(e) if e.is_busy() => Err(e),
                        answer => Ok(answer),
                    }
                })
            };
            let answer = match tokio::time::timeout(self.timeout, retry_busy(attempt)).await {
                Err(_) => {
                    let detail = format!("no answer within {} s", self.timeout.as_secs_f32());
                    return Err(ImageError::new(ImageErrorKind::Timeout, TIMEOUT, detail));
                }
                Ok(Err(still_busy)) => {
                    return Err(ImageError::new(
                        ImageErrorKind::Busy,
                        BUSY,
                        still_busy.detail,
                    ));
                }
                Ok(Ok(answer)) => answer,
            };
            let images = answer.map_err(|e| image_error(&e, client.is_relay()))?;
            if images.is_empty() {
                return Err(ImageError::new(
                    ImageErrorKind::Other,
                    NO_PICTURE,
                    "the answer held no image",
                ));
            }
            Ok(images.into_iter().map(made).collect())
        })
    }
}

fn made(image: GeneratedImage) -> MadeImage {
    MadeImage {
        png: image.png,
        revised_prompt: image.revised_prompt,
    }
}

/// A final Meta client error as one plain sentence about Muse Image; its `Display` (already redacted) for logs.
fn image_error(e: &MetaError, relay: bool) -> ImageError {
    let (kind, message) = match e {
        MetaError::Budget { .. } if relay => (ImageErrorKind::OutOfCredit, RELAY_OUT_OF_CREDIT),
        MetaError::Budget { .. } => (ImageErrorKind::OutOfCredit, KEY_OUT_OF_CREDIT),
        // Meta's own rate limit or a moment it's overloaded: the person can try again soon.
        MetaError::RateLimited { .. } | MetaError::Overloaded { .. } => {
            (ImageErrorKind::Busy, BUSY)
        }
        MetaError::Refused { .. } => (ImageErrorKind::Refused, REFUSED),
        MetaError::Unauthorized | MetaError::NotAllowed { .. } => {
            (ImageErrorKind::Other, NOT_ALLOWED)
        }
        MetaError::Network(_) => (ImageErrorKind::Other, OFFLINE),
        MetaError::BadRequest { .. }
        | MetaError::Decode(_)
        | MetaError::LocalFile { .. }
        | MetaError::AlignUnavailable => (ImageErrorKind::Other, FAILED),
    };
    ImageError::new(kind, message, e.to_string())
}
