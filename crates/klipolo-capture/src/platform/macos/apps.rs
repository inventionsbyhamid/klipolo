//! The running apps for the app picker, and which look like games (detection only suggests, never starts anything). Read
//! without capture or prompts:
//! - `NSWorkspace`'s running apps with the regular activation policy (windows and a Dock icon), Klipolo left out;
//! - each bundle's `Info.plist` through `NSBundle`: `LSApplicationCategoryType`, `GCSupportsGameMode` and
//!   `LSSupportsGameMode`. A bundle in a folder macOS guards (Desktop, Documents, Downloads, iCloud Drive, cloud
//!   storage, other apps' containers, other volumes) is never read, since reading it could ask the person for access:
//!   there only its executable's path counts (a Steam library, `games`).
//!
//! Games come first, then by name. NSBundle keeps each bundle it opened for the life of the process: a few KB per app
//! ever listed.

use std::collections::BTreeMap;
use std::path::Path;

use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};
use objc2_foundation::{NSBundle, NSNumber, NSString, NSURL};

use crate::backend::AppInfo;
use crate::games::looks_like_game;
use crate::platform::common::mac_preflight::{PLIST_KEYS, app_facts, reads_without_asking};

pub(super) fn running() -> Vec<AppInfo> {
    let own = std::process::id();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let mut apps: Vec<AppInfo> = objc2::rc::autoreleasepool(|_| {
        let running = NSWorkspace::sharedWorkspace().runningApplications();
        (running.iter())
            .filter(|a| a.activationPolicy() == NSApplicationActivationPolicy::Regular)
            .filter(|a| u32::try_from(a.processIdentifier()).is_ok_and(|pid| pid != own))
            .filter_map(|a| app(&a, home.as_deref()))
            .collect()
    });
    apps.sort_by_cached_key(|a| (!a.is_game, a.name.to_lowercase()));
    apps
}

/// A running app's id (`AppInfo::app_id`): its bundle id, else its executable's path.
pub(super) fn app_id(a: &NSRunningApplication) -> Option<String> {
    (a.bundleIdentifier().map(|id| id.to_string())).or_else(|| exe_of(a))
}

/// The id of the running app with process `pid` (`app_id`), if it is one.
pub(super) fn app_id_of_pid(pid: i32) -> Option<String> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid).and_then(|a| app_id(&a))
}

fn exe_of(a: &NSRunningApplication) -> Option<String> {
    (a.executableURL().and_then(|u| u.to_file_path())).map(|p| p.to_string_lossy().into_owned())
}

fn app(a: &NSRunningApplication, home: Option<&Path>) -> Option<AppInfo> {
    let exe = exe_of(a);
    let app_id = app_id(a)?;
    let bundle = a.bundleURL();
    let readable = (bundle.as_deref().and_then(NSURL::to_file_path))
        .is_some_and(|path| home.is_some_and(|home| reads_without_asking(&path, home)));
    let plist = match bundle {
        Some(url) if readable => info_plist(&url),
        _ => BTreeMap::new(),
    };
    let facts = app_facts(&app_id, &plist, exe.as_deref());
    let name = (a.localizedName().map(|n| n.to_string()))
        .or_else(|| exe.as_deref().and_then(file_name))
        .unwrap_or_else(|| app_id.clone());
    Some(AppInfo {
        is_game: looks_like_game(&facts),
        app_id,
        name,
        pid: u32::try_from(a.processIdentifier()).unwrap_or(0),
        frontmost: a.isActive(),
        exe,
    })
}

/// The bundle's `PLIST_KEYS`, as text.
fn info_plist(bundle: &NSURL) -> BTreeMap<String, String> {
    let Some(info) = NSBundle::bundleWithURL(bundle).and_then(|b| b.infoDictionary()) else {
        return BTreeMap::new();
    };
    (PLIST_KEYS.iter())
        .filter_map(|&key| {
            let value = info.objectForKey(&NSString::from_str(key))?;
            let text = if let Some(s) = value.downcast_ref::<NSString>() {
                s.to_string()
            } else {
                u8::from(value.downcast_ref::<NSNumber>()?.boolValue()).to_string()
            };
            Some((key.to_owned(), text))
        })
        .collect()
}

fn file_name(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}
