//! The three permissions' states, read without prompting:
//! - screen recording: `CGPreflightScreenCaptureAccess`, true is Granted. False can't tell "denied" from "not asked"
//!   without a prompt, so it is NotAsked: the recorder shows its Screen Recording step before Record, whose press asks
//!   (`screen_permission`).
//! - camera and microphone: AVFoundation's authorization status, NotAsked, Denied (denied or restricted) or Granted.

use cidre::{av, objc};
use objc2_core_graphics::CGPreflightScreenCaptureAccess;

use crate::backend::{Permission, Permissions};

pub(super) fn read() -> Permissions {
    Permissions {
        screen: if CGPreflightScreenCaptureAccess() {
            Permission::Granted
        } else {
            Permission::NotAsked
        },
        camera: status(av::MediaType::video()),
        mic: status(av::MediaType::audio()),
    }
}

/// The microphone's state alone, in an autorelease pool of its own: the Record dialog's meter reads it once a second on
/// a thread with no run loop to drain one.
pub(super) fn mic() -> Permission {
    objc::ar_pool(|| status(av::MediaType::audio()))
}

/// AVFoundation's answer for `media`; Unknown when it raises.
fn status(media: &av::MediaType) -> Permission {
    match av::CaptureDevice::authorization_status_for_media_type(media) {
        Ok(av::AuthorizationStatus::Authorized) => Permission::Granted,
        Ok(av::AuthorizationStatus::NotDetermined) => Permission::NotAsked,
        Ok(av::AuthorizationStatus::Denied | av::AuthorizationStatus::Restricted) => {
            Permission::Denied
        }
        Err(_) => Permission::Unknown,
    }
}
