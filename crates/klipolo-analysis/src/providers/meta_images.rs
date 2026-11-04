//! Muse Image behind `ImageMaker`, through the one shared `Meta`: each request waits a while for one of the device's
//! Meta slots (`Meta::call`; a clip's analysis can hold them for minutes), then gets its own deadline, and is sent again
//! while the relay or Traefik says busy (`retry_busy`). A request that never gets a slot is busy: nothing was sent, so
//! nothing was spent. Whether the client calls Meta with the person's key or goes through the relay is the client's
//! business. Meta's types and errors stop here: each failure becomes one plain sentence.

use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::future::{self, Either};
use klipolo_meta::{GeneratedImage, ImageEffort, MetaClient, MetaError};

use super::BoxFuture;
use super::images::{ImageError, ImageErrorKind, ImageMaker, ImageSize, MadeImage};
use super::meta::Meta;
use super::{ProviderError, retry_busy};

/// What the library calls the pictures this makes: "Made with Muse Image".
pub const MADE_WITH: &str = "Muse Image";

/// The longest Muse Image may take to answer once a request holds a slot. A real sticker took 34.7 s.
pub const IMAGE_TIMEOUT: Duration = Duration::from_secs(120);

/// The longest a request waits for one of the device's slots before it is called busy.
pub const QUEUE_WAIT: Duration = Duration::from_secs(60);

const BUSY: &str = "Muse Image is busy right now, so try again in a moment.";
const RELAY_OUT_OF_CREDIT: &str = "Klipolo's credit for Muse Image has run out, so add your own Meta key in Settings to keep going.";
const KEY_OUT_OF_CREDIT: &str =
    "Your Meta account is out of credit or has a billing problem, so check it on Meta's site.";
const REFUSED: &str = "Muse Image won't draw that, so try describing it another way.";
const TIMEOUT: &str = "Muse Image took too long to answer, so try again.";
const NOT_ALLOWED: &str = "Your Meta key or access code wasn't accepted, so check it in Settings.";
const NOT_CONFIGURED: &str = "Klipolo's AI service isn't set up yet, so try again later or add your own Meta key in Settings.";
const OFFLINE: &str =
    "Klipolo couldn't reach Muse Image, so check your internet connection and try again.";
const NO_PICTURE: &str = "Muse Image didn't send a picture back, so try again.";
const FAILED: &str = "Muse Image couldn't make that picture, so try again.";

/// Muse Image through one device's shared `Meta` (clones share its slots).
#[derive(Clone, Debug)]
pub struct MetaImages {
    meta: Meta,
    timeout: Duration,
    queue_wait: Duration,
    effort: ImageEffort,
}

impl MetaImages {
    /// Muse Image at its full effort, within `IMAGE_TIMEOUT`.
    pub fn new(meta: Meta) -> Self {
        Self {
            meta,
            timeout: IMAGE_TIMEOUT,
            queue_wait: QUEUE_WAIT,
            effort: ImageEffort::Full,
        }
    }

    /// The longest a request may take.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The longest a request waits for a slot.
    pub fn with_queue_wait(mut self, wait: Duration) -> Self {
        self.queue_wait = wait;
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
            let size = match size {
                ImageSize::Square => klipolo_meta::ImageSize::Square,
                ImageSize::Landscape => klipolo_meta::ImageSize::Landscape,
                ImageSize::Portrait => klipolo_meta::ImageSize::Portrait,
            };
            let images = match retry_busy(|| self.attempt(prompt, size, n)).await {
                Ok(answer) => answer?,
                Err(still_busy) => {
                    return Err(ImageError::new(
                        ImageErrorKind::Busy,
                        BUSY,
                        still_busy.detail,
                    ));
                }
            };
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

impl MetaImages {
    /// One request: it waits at most `queue_wait` for a slot (nothing is sent meanwhile), then Muse Image has `timeout`
    /// to answer. Only a busy answer comes out as an error, for `retry_busy` to send again; any other answer, failures
    /// included, comes out as it is, so a final failure is never asked again on its own.
    async fn attempt(
        &self,
        prompt: &str,
        size: klipolo_meta::ImageSize,
        n: u8,
    ) -> Result<Result<Vec<GeneratedImage>, ImageError>, ProviderError> {
        let client = self.meta.client();
        let holds_slot = AtomicBool::new(false);
        let call = pin!(self.meta.call(async {
            // `call` runs this once it holds a slot: the deadline starts here.
            holds_slot.store(true, Ordering::Release);
            let request = client.generate_image_with(prompt, size, n, self.effort);
            match tokio::time::timeout(self.timeout, request).await {
                Err(_) => {
                    let detail = format!("no answer within {} s", self.timeout.as_secs_f32());
                    Ok(Err(ImageError::new(
                        ImageErrorKind::Timeout,
                        TIMEOUT,
                        detail,
                    )))
                }
                Ok(Err(e)) if e.is_busy() => Err(e),
                Ok(Err(e)) => Ok(Err(image_error(&e, client))),
                Ok(Ok(images)) => Ok(Ok(images)),
            }
        }));
        let queue = pin!(tokio::time::sleep(self.queue_wait));
        match future::select(call, queue).await {
            Either::Left((answer, _)) => answer,
            // Still waiting for a slot: give up before anything is sent.
            Either::Right(((), _)) if !holds_slot.load(Ordering::Acquire) => {
                let detail = format!("no slot within {} s", self.queue_wait.as_secs_f32());
                Ok(Err(ImageError::new(ImageErrorKind::Busy, BUSY, detail)))
            }
            // Already sent: its own deadline decides.
            Either::Right(((), call)) => call.await,
        }
    }
}

fn made(image: GeneratedImage) -> MadeImage {
    MadeImage {
        png: image.png,
        revised_prompt: image.revised_prompt,
    }
}

/// A final Meta client error from `client` as one plain sentence about Muse Image; its `Display` (already redacted) for
/// logs. A used-up credit the client was told how to say (`MetaClient::with_used_up_message`) says that.
fn image_error(e: &MetaError, client: &MetaClient) -> ImageError {
    let relay = client.is_relay();
    let (kind, message) = match e {
        MetaError::Budget { message } if client.used_up_message() == Some(message.as_str()) => {
            (ImageErrorKind::OutOfCredit, message.as_str())
        }
        MetaError::Budget { .. } if relay => (ImageErrorKind::OutOfCredit, RELAY_OUT_OF_CREDIT),
        MetaError::Budget { .. } => (ImageErrorKind::OutOfCredit, KEY_OUT_OF_CREDIT),
        // Meta's own rate limit or a moment it's overloaded: the person can try again soon.
        MetaError::RateLimited { .. } | MetaError::Overloaded { .. } => {
            (ImageErrorKind::Busy, BUSY)
        }
        MetaError::Refused { .. } => (ImageErrorKind::Refused, REFUSED),
        MetaError::TimedOut { .. } => (ImageErrorKind::Timeout, TIMEOUT),
        MetaError::NotConfigured => (ImageErrorKind::Other, NOT_CONFIGURED),
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
