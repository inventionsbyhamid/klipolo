//! What the parent hands the render worker beside a request: files and folders it wrote itself into the
//! session's `in/` (shaders, scenes, SVGs, a project snapshot, contributions, image assets, video stills it decoded),
//! named by plain names. The worker resolves a name only against its own `in/`; no name is a path.

use serde::{Deserialize, Serialize};

/// The worker's profile in `in/`, which the parent writes before the launch and no input may replace.
pub const PROFILE_NAME: &str = "profile.json";
const MAX_NAME_BYTES: usize = 128;

/// A plain file or folder name in the session's `in/`: 1–128 bytes of ASCII letters, digits, `.`, `-` and `_`, not
/// starting with `.` (so never `.`, `..` or a hidden name), never [`PROFILE_NAME`] or the worker's `fonts.json`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InputName(String);

impl InputName {
    pub fn new(name: &str) -> Result<Self, String> {
        let plain = name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
        if name.is_empty() || name.len() > MAX_NAME_BYTES || !plain || name.starts_with('.') {
            return Err(format!("{name:?} isn't a plain input name"));
        }
        if name == PROFILE_NAME || name == crate::make::fonts::FONTS_NAME {
            return Err(format!("{name} is the worker's own"));
        }
        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for InputName {
    type Error = String;

    fn try_from(name: String) -> Result<Self, String> {
        Self::new(&name)
    }
}

impl From<InputName> for String {
    fn from(name: InputName) -> String {
        name.0
    }
}

/// One input: a file, or a folder of files one level deep (`stills/`, a `Verify` folder, a contributions folder). Not
/// `Clone`: what goes to the worker is written once and moved, never copied to be held.
#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    File(InputName, Vec<u8>),
    Folder(InputName, Vec<(InputName, Vec<u8>)>),
}
