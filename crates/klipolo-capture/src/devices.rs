//! What a computer can record and may record, read without capture or prompts (device inspection never
//! activates capture). Each OS lists these from its own registries, never through cpal's listing.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What can be recorded, listed without capture.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Devices {
    pub displays: Vec<DisplayInfo>,
    pub apps: Vec<AppInfo>,
    pub cameras: Vec<DeviceInfo>,
    pub mics: Vec<DeviceInfo>,
    /// Where the computer's sound goes now (a Bluetooth headset's output makes preflight pick the built-in mic).
    pub output: Option<DeviceInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DisplayInfo {
    pub id: String,
    pub name: String,
    /// The display's own pixels.
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
}

/// A running app with windows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppInfo {
    /// A bundle id (macOS), an executable path (Windows), a desktop-file id or executable (Linux).
    pub app_id: String,
    pub name: String,
    pub pid: u32,
    pub is_game: bool,
    pub frontmost: bool,
    pub exe: Option<String>,
}

/// A camera, a mic or the sound output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceInfo {
    pub id: String,
    /// The device's own name ("MV7"): user text.
    pub name: String,
    pub transport: Transport,
    /// A virtual device (BlackHole, Teams): it may carry no voice.
    pub is_virtual: bool,
    /// Another app holds it (cameras).
    pub in_use: bool,
    /// Turned off or covered (cameras).
    pub suspended: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Transport {
    BuiltIn,
    Usb,
    Bluetooth,
    /// An iPhone used as a camera or mic.
    Continuity,
    Virtual,
    Other,
}

/// The OS's answer for each kind of capture, read without prompting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Permissions {
    pub screen: Permission,
    pub camera: Permission,
    pub mic: Permission,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Permission {
    Granted,
    /// The first Record asks.
    NotAsked,
    Denied,
    /// This OS doesn't ask (or asks per recording through its own picker).
    NotNeeded,
    #[default]
    Unknown,
}
