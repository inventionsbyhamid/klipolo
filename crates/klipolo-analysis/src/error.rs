//! Why an analysis step stopped, and the sentence shown for it.

use crate::audio::AudioError;
use crate::providers::ProviderError;

#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    /// The clip's WAV couldn't be read or sliced.
    #[error(transparent)]
    Audio(#[from] AudioError),
    /// The clip's picture couldn't be copied small or read for its cuts.
    #[error(transparent)]
    Video(#[from] VideoError),
    /// A service refused or failed a request the step can't do without.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

impl AnalysisError {
    /// The plain sentence to show the person.
    pub fn user_message(&self) -> String {
        match self {
            Self::Audio(_) => "Klipolo could not read this clip's sound.".into(),
            Self::Video(e) => e.user_message().into(),
            Self::Provider(e) => e.user_message().into(),
        }
    }

    /// A budget or quota stop (`ProviderError::is_stop`).
    pub fn is_stop(&self) -> bool {
        matches!(self, Self::Provider(e) if e.is_stop())
    }
}

/// Why the scene log's small copy of a clip, or its cuts, couldn't be made. The messages name no file.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VideoError {
    /// The clip has no picture (sound only).
    #[error("the clip has no picture")]
    NoVideo,
    /// This computer has no H.264 encoder for the small copy (Linux, for now).
    #[error("no H.264 encoder")]
    NoEncoder,
    /// Stopped by its cancel flag.
    #[error("stopped")]
    Stopped,
    /// Reading, decoding or writing failed; the text is for logs.
    #[error("{0}")]
    Failed(String),
}

impl VideoError {
    /// The plain sentence to show the person.
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::NoVideo => "This clip has no picture to watch.",
            Self::NoEncoder => "This computer can't make the small copy the scene log needs.",
            Self::Stopped => "Stopped.",
            Self::Failed(_) => "Klipolo could not read this clip's picture.",
        }
    }
}
