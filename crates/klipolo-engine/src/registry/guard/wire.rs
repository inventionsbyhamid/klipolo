//! Diagnostics as the app reads them from the render worker's reply lines: a [`super::Diagnostic`]'s
//! `code` is a `&'static str` the guard names, so the parent keeps each code it reads once, for the run. Codes have the
//! guard's shape (`scan.size`, `rule.shadow`: a lowercase word, then dot-separated words of letters and digits, at
//! most 48 bytes), and the parent keeps at most [`MAX_CODES`], so a worker that sends codes it made up can't grow the
//! parent's memory: past the cap its reply is refused.

use std::collections::BTreeSet;
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, de::Error};

/// Distinct diagnostic codes the parent keeps; the guard has a few dozen.
const MAX_CODES: usize = 256;
const MAX_CODE_BYTES: usize = 48;

static CODES: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

/// `#[serde(deserialize_with)]` for `Diagnostic::code`.
pub(super) fn code<'de, D: Deserializer<'de>>(deserializer: D) -> Result<&'static str, D::Error> {
    let text = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
    keep(&text).ok_or_else(|| D::Error::custom("not a diagnostic code"))
}

fn keep(code: &str) -> Option<&'static str> {
    if !well_formed(code) {
        return None;
    }
    let mut codes = CODES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(kept) = codes.get(code) {
        return Some(kept);
    }
    if codes.len() >= MAX_CODES {
        return None;
    }
    let kept: &'static str = Box::leak(code.to_owned().into_boxed_str());
    codes.insert(kept);
    Some(kept)
}

fn well_formed(code: &str) -> bool {
    let mut words = code.split('.');
    let first = words.next().unwrap_or("");
    code.len() <= MAX_CODE_BYTES
        && code.contains('.')
        && !first.is_empty()
        && first.bytes().all(|b| b.is_ascii_lowercase())
        && words.all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_have_the_guards_shape_and_are_kept_once() {
        for good in [
            "scan.size",
            "rule.shadow",
            "harden.roundtrip",
            "wgsl.parse",
            "scan.newline2",
        ] {
            assert_eq!(keep(good), Some(good));
        }
        assert!(std::ptr::eq(
            keep("scan.size").unwrap(),
            keep("scan.size").unwrap()
        ));
        for bad in [
            "",
            "scan",
            ".size",
            "scan.",
            "Scan.size",
            "scan..size",
            "../etc",
            "scan.si ze",
            "scan.size\n",
        ] {
            assert_eq!(keep(bad), None, "{bad:?}");
        }
        assert_eq!(keep(&format!("scan.{}", "a".repeat(48))), None, "too long");
    }
}
