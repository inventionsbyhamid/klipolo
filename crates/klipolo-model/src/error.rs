use std::fmt;

use crate::coded::{english, CodedError};
use crate::messages::MESSAGES;

#[derive(Debug, Clone, PartialEq)]
pub enum ModelError {
    OlderVersion,
    UnsupportedVersion(u32),
    /// serde's description, for the log only (the person reads `model.unreadable`).
    Parse(String),
    Invalid(CodedError),
    /// A newer Klipolo's file this build can't read or check; holds the requirements it lacks, for the log.
    NewerFeatures(String),
    /// `kind`: "clip", "layer", "track", "media", "lut", "effect" or "recording".
    NotFound {
        kind: &'static str,
        id: String,
    },
}

impl ModelError {
    /// What the person is told, as a code.
    pub fn coded(&self) -> CodedError {
        match self {
            Self::OlderVersion => CodedError::new("model.olderVersion"),
            Self::UnsupportedVersion(_) | Self::NewerFeatures(_) => {
                CodedError::new("model.newerVersion")
            }
            Self::Parse(_) => CodedError::new("model.unreadable"),
            Self::Invalid(e) => e.clone(),
            Self::NotFound { kind, .. } => CodedError::new(match *kind {
                "clip" => "model.notFound.clip",
                "track" => "model.notFound.track",
                "media" => "model.notFound.media",
                "lut" => "model.notFound.lut",
                "effect" => "model.notFound.effect",
                "recording" => "model.notFound.recording",
                _ => "model.notFound.layer",
            }),
        }
    }
}

/// English, for logs and MCP replies; the UI words `coded()`.
impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&english(&[MESSAGES], &self.coded()))
    }
}

impl std::error::Error for ModelError {}

pub type Result<T> = std::result::Result<T, ModelError>;

/// A refusal with no values; the message is built only on failure.
pub(crate) fn invalid(code: &'static str) -> ModelError {
    ModelError::Invalid(CodedError::new(code))
}

pub(crate) fn invalid_with(e: CodedError) -> ModelError {
    ModelError::Invalid(e)
}

/// `kind`: "clip", "layer", "track", "media", "lut", "effect" or "recording".
pub(crate) fn not_found(kind: &'static str, id: &str) -> ModelError {
    ModelError::NotFound {
        kind,
        id: id.to_string(),
    }
}
