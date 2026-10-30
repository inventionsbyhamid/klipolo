//! The fakes of the seams updates and uploads ask about: a network path, Low Power Mode, a wake and an install
//! place to order, each winning over the OS's answer until cleared (process-wide, as `set_thermal`).

use std::sync::atomic::{AtomicU8, Ordering};

use crate::install::InstallPlace;
use crate::network::NetPath;

/// The network path `network::path` reports while set (`SET` with the path's three bits; 0: the OS's own).
static NET_PATH: AtomicU8 = AtomicU8::new(0);
const SET: u8 = 0x80;

/// Makes `network::path` report `path` (None: the OS's own again). A new path also reaches `network::on_change`'s
/// listeners, as the OS's would.
pub fn set_net_path(path: Option<NetPath>) {
    NET_PATH.store(path.map_or(0, |path| SET | path.bits()), Ordering::SeqCst);
    if let Some(path) = path {
        crate::network::notify(path);
    }
}

/// The path `set_net_path` set, if any.
pub fn net_path_override() -> Option<NetPath> {
    let code = NET_PATH.load(Ordering::SeqCst);
    (code & SET != 0).then(|| NetPath::from_bits(code & !SET))
}

/// Low Power Mode as `power::low_power` reports it while set (0: the OS's own; 1 off; 2 on).
static LOW_POWER: AtomicU8 = AtomicU8::new(0);

/// Makes `power::low_power` report `on` (None: the OS's own again).
pub fn set_low_power(on: Option<bool>) {
    LOW_POWER.store(on.map_or(0, |on| 1 + u8::from(on)), Ordering::SeqCst);
}

/// The answer `set_low_power` set, if any.
pub fn low_power_override() -> Option<bool> {
    match LOW_POWER.load(Ordering::SeqCst) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

/// Every `power::on_wake` listener runs, as after a real wake.
pub fn wake() {
    crate::power::woke();
}

/// The place `install::place` reports while set (0: the OS's own; else 1 + its index in `PLACES`).
static INSTALL_PLACE: AtomicU8 = AtomicU8::new(0);
const PLACES: [InstallPlace; 9] = [
    InstallPlace::Writable,
    InstallPlace::NeedsAdmin,
    InstallPlace::NonAtomic,
    InstallPlace::Translocated,
    InstallPlace::DiskImage,
    InstallPlace::Locked,
    InstallPlace::Network,
    InstallPlace::Managed,
    InstallPlace::Unknown,
];

/// Makes `install::place` report `place` for any bundle (None: the OS's own again).
pub fn set_install_place(place: Option<InstallPlace>) {
    let code = place
        .and_then(|place| PLACES.iter().position(|&known| known == place))
        .map_or(0, |index| index as u8 + 1);
    INSTALL_PLACE.store(code, Ordering::SeqCst);
}

/// The place `set_install_place` set, if any.
pub fn install_place_override() -> Option<InstallPlace> {
    let code = usize::from(INSTALL_PLACE.load(Ordering::SeqCst));
    code.checked_sub(1)
        .and_then(|index| PLACES.get(index).copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    #[test]
    fn the_seams_fakes_win_until_cleared_and_their_listeners_hear_them() {
        // A metered path is reported, and heard as a change.
        let heard = Arc::new(Mutex::new(Vec::new()));
        let sink = heard.clone();
        crate::network::on_change(move |path| sink.lock().unwrap().push(path));
        let metered = NetPath {
            online: true,
            constrained: true,
            expensive: false,
        };
        set_net_path(Some(metered));
        assert_eq!(crate::network::path(), metered);
        assert!(heard.lock().unwrap().contains(&metered));
        set_net_path(None);
        assert_eq!(net_path_override(), None);

        set_low_power(Some(true));
        assert!(crate::power::low_power());
        set_low_power(Some(false));
        assert!(!crate::power::low_power());
        set_low_power(None);
        assert_eq!(low_power_override(), None);

        let woke = Arc::new(AtomicBool::new(false));
        let flag = woke.clone();
        crate::power::on_wake(move || flag.store(true, Ordering::SeqCst));
        wake();
        assert!(woke.load(Ordering::SeqCst));

        for place in PLACES {
            set_install_place(Some(place));
            assert_eq!(
                crate::install::place(Path::new("/nowhere/Klipolo.app")),
                place
            );
        }
        set_install_place(None);
        assert_eq!(install_place_override(), None);
    }
}
