//! Pictures drawn from a prompt (stickers, title cards), behind an interface Klipolo owns: an `ImageMaker` draws PNGs.
//! Its types and errors are Klipolo's own, so no vendor's type reaches the app: each vendor lives in an adapter that
//! implements the trait and maps its types and errors (`meta_images` today). Tests use fakes of the trait.

use std::fmt;

use super::BoxFuture;

/// The picture's shape: the aspect ratio asked for, not a pixel size (the service picks that).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageSize {
    Square,
    Landscape,
    Portrait,
}

/// One picture: PNG bytes (opaque: no service promises transparency), and the prompt the service actually drew from
/// when it said.
#[derive(Clone, PartialEq, Eq)]
pub struct MadeImage {
    pub png: Vec<u8>,
    pub revised_prompt: Option<String>,
}

/// The size, not the bytes.
impl fmt::Debug for MadeImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MadeImage")
            .field("png", &format_args!("{} bytes", self.png.len()))
            .field("revised_prompt", &self.revised_prompt)
            .finish()
    }
}

/// What kind of failure a picture request met. Every kind leaves nothing behind; the person may Try again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageErrorKind {
    /// Busy or overloaded for now: trying again in a moment can work.
    Busy,
    /// The credit for these calls is used up: trying again won't help until the person acts.
    OutOfCredit,
    /// The service won't draw this prompt: other words may work.
    Refused,
    /// No answer in time.
    Timeout,
    /// Anything else (refused credentials, unreachable, an unreadable answer).
    Other,
}

/// A picture request's failure. `message` is one plain sentence to show the person; `detail` is for logs. Neither
/// holds a key or token: adapters build them from text that is already safe.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{detail}")]
pub struct ImageError {
    pub kind: ImageErrorKind,
    pub message: String,
    pub detail: String,
}

impl ImageError {
    pub fn new(
        kind: ImageErrorKind,
        message: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            detail: detail.into(),
        }
    }

    /// The plain sentence to show the person.
    pub fn user_message(&self) -> &str {
        &self.message
    }
}

/// Prompt to PNGs.
pub trait ImageMaker: Send + Sync {
    /// The model's name for people ("Muse Image"), for the label on what it made ("Made with Muse Image").
    fn made_with(&self) -> &str;

    /// `n` pictures of `prompt` in the shape `size`. The answer may hold fewer than `n`; an empty one is an error.
    fn make<'a>(
        &'a self,
        prompt: &'a str,
        size: ImageSize,
        n: u8,
    ) -> BoxFuture<'a, Result<Vec<MadeImage>, ImageError>>;
}
