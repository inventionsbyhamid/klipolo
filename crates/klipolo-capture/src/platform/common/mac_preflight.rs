//! The macOS preflight's pure rules (`platform::macos` reads the OS and calls these):
//! - `transport_of`: a device's transport from Core Audio's (and AVFoundation's) transport code and its name;
//! - `app_facts`: what game detection needs from an app's bundle id, `Info.plist` keys and executable;
//! - `reads_without_asking`: whether a bundle's `Info.plist` can be read without macOS asking the person for access.

use std::collections::BTreeMap;
use std::path::Path;

use crate::backend::Transport;
use crate::games::{AppFacts, contains_ignore_case};

/// Devices whose transport doesn't say they are virtual (a driver may report USB or built-in), by a part of their
/// name in any case: loopback and meeting drivers, noise removers, virtual cameras.
const VIRTUAL_NAMES: [&str; 14] = [
    "BlackHole",
    "Soundflower",
    "Loopback Audio",
    "Microsoft Teams Audio",
    "ZoomAudioDevice",
    "Krisp",
    "Background Music",
    "eqMac",
    "VB-Cable",
    "Audio Hijack",
    "NVIDIA Broadcast",
    "OBS Virtual Camera",
    "Snap Camera",
    "ManyCam",
];

/// The `Info.plist` keys game detection reads.
pub const PLIST_KEYS: [&str; 3] = [
    "LSApplicationCategoryType",
    "GCSupportsGameMode",
    "LSSupportsGameMode",
];

/// Folders under the home folder whose files macOS guards behind a prompt.
const GUARDED: [&str; 7] = [
    "Desktop",
    "Documents",
    "Downloads",
    "Library/Mobile Documents",
    "Library/CloudStorage",
    "Library/Containers",
    "Library/Group Containers",
];

/// A device's transport from Core Audio's `kAudioDeviceTransportType*` code (AVFoundation's `transportType` uses the
/// same codes) and its name: a known virtual device by name whatever it reports.
pub fn transport_of(core_audio_transport: u32, name: &str) -> Transport {
    if VIRTUAL_NAMES.iter().any(|v| contains_ignore_case(name, v)) {
        return Transport::Virtual;
    }
    match &core_audio_transport.to_be_bytes() {
        b"bltn" => Transport::BuiltIn,
        b"usb " => Transport::Usb,
        b"blue" | b"blea" => Transport::Bluetooth,
        b"virt" => Transport::Virtual,
        // An iPhone's mic or camera through Continuity, by cable or wireless.
        b"ccwd" | b"ccwl" => Transport::Continuity,
        _ => Transport::Other,
    }
}

/// What `looks_like_game` needs from an app's bundle id, its `Info.plist` (`PLIST_KEYS`, as text: a boolean is "1" or
/// "0") and its executable's path.
pub fn app_facts(
    bundle_id: &str,
    info_plist: &BTreeMap<String, String>,
    exe: Option<&str>,
) -> AppFacts {
    let yes = |key: &str| {
        info_plist.get(key).is_some_and(|v| {
            ["1", "true", "yes"]
                .iter()
                .any(|t| v.eq_ignore_ascii_case(t))
        })
    };
    AppFacts {
        app_id: bundle_id.to_owned(),
        category: info_plist.get(PLIST_KEYS[0]).cloned(),
        game_mode: yes(PLIST_KEYS[1]) || yes(PLIST_KEYS[2]),
        exe: exe.map(str::to_owned),
        desktop_categories: Vec::new(),
    }
}

/// Whether a bundle at `bundle` can be read without macOS asking for access, `home` being the home folder: not in a
/// guarded folder (Desktop, Documents, Downloads, iCloud Drive, cloud storage, other apps' containers), nor on another
/// volume, nor translocated (Gatekeeper runs a quarantined app, such as a game started from Downloads, from a random
/// `…/AppTranslocation/…` copy, whose reading is unverified: treated as guarded).
pub fn reads_without_asking(bundle: &Path, home: &Path) -> bool {
    let guarded = GUARDED.iter().any(|g| bundle.starts_with(home.join(g)));
    let translocated = bundle
        .components()
        .any(|c| c.as_os_str() == "AppTranslocation");
    !guarded && !translocated && !bundle.starts_with("/Volumes")
}
