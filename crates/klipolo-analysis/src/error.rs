//! Why an analysis step stopped, and the sentence shown for it.

use crate::audio::AudioError;
use crate::providers::ProviderError;

#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    /// The clip's WAV couldn't be read or sliced.
    #[error(transparent)]
    Audio(#[from] AudioError),
    /// A service refused or failed a request the step can't do without.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

impl AnalysisError {
    /// The plain sentence to show the person.
    pub fn user_message(&self) -> String {
        match self {
            Self::Audio(_) => "Klipolo could not read this clip's sound.".into(),
            Self::Provider(e) => e.user_message().into(),
        }
    }

    /// A budget or quota stop (`ProviderError::is_stop`).
    pub fn is_stop(&self) -> bool {
        matches!(self, Self::Provider(e) if e.is_stop())
    }
}
