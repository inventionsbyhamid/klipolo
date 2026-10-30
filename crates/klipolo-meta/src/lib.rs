//! One HTTP client for Meta's Model API, used directly with the person's own
//! key or through Klipolo's relay with a device token from an access code:
//! Voice Transcribe, the Responses API (Muse Spark, SAM), the Files API and
//! Muse Image.
//!
//! Times stay in the API's units (milliseconds); callers convert.

mod asr;
mod client;
mod error;
mod files;
mod images;
mod ledger;
mod relay;
mod responses;

pub mod pricing;

pub use asr::{AsrResult, AsrTurn, TRANSCRIBE_MODEL};
pub use client::{
    meta_base, relay_base, Auth, MetaClient, DEFAULT_META_BASE, DEFAULT_RELAY_BASE, META_BASE_ENV,
    RELAY_BASE_ENV,
};
pub use error::MetaError;
pub use images::{GeneratedImage, ImageEffort, ImageSize, IMAGE_MODEL};
pub use ledger::SPEND_LEDGER_ENV;
pub use relay::{AlignResult, AlignTurn, AlignedWord, DeviceInfo};
pub use responses::{FunctionCall, ResponseOut, Usage};
pub use secrecy::{ExposeSecret, SecretString};
