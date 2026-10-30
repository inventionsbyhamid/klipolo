//! The battery from sysfs (`power::battery`): each `/sys/class/power_supply/BAT*` folder's `capacity` (0–100) and
//! `status` ("Charging", "Discharging", "Full", "Not charging", "Unknown"). A mouse's or a phone's battery has another
//! name (`hidpp_battery_0`), so only the computer's are read.

use std::path::Path;

use crate::power::Battery;

const POWER_SUPPLY: &str = "/sys/class/power_supply";

pub(crate) fn battery() -> Option<Battery> {
    battery_in(Path::new(POWER_SUPPLY))
}

/// The batteries under `root` together: their average charge, and charging unless one of them is discharging (a
/// laptop with two drains one at a time while the other reads "Unknown" or "Not charging"). None without a battery
/// whose charge reads.
fn battery_in(root: &Path) -> Option<Battery> {
    let (mut sum, mut count, mut draining) = (0u32, 0u32, false);
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("BAT") {
            continue;
        }
        let dir = entry.path();
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
        let Some(percent) = read("capacity").and_then(|c| c.trim().parse::<u8>().ok()) else {
            continue;
        };
        sum += u32::from(percent.min(100));
        count += 1;
        draining |= read("status").is_some_and(|s| s.trim() == "Discharging");
    }
    (count > 0).then(|| Battery {
        percent: (sum / count) as u8,
        charging: !draining,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batteries_are_read_together_and_other_supplies_are_not() {
        let root = tempfile::tempdir().unwrap();
        let supply = |name: &str, files: &[(&str, &str)]| {
            let dir = root.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            for (file, text) in files {
                std::fs::write(dir.join(file), text).unwrap();
            }
        };
        assert_eq!(battery_in(root.path()), None, "a desktop");
        supply("AC", &[("online", "1\n")]);
        supply(
            "hidpp_battery_0",
            &[("capacity", "5\n"), ("status", "Discharging\n")],
        );
        supply("BAT0", &[("capacity", "15\n"), ("status", "Discharging\n")]);
        supply("BAT1", &[("capacity", "25\n"), ("status", "Unknown\n")]);
        assert_eq!(
            battery_in(root.path()),
            Some(Battery {
                percent: 20,
                charging: false
            })
        );
        std::fs::write(root.path().join("BAT0/status"), "Not charging\n").unwrap();
        assert_eq!(
            battery_in(root.path()),
            Some(Battery {
                percent: 20,
                charging: true
            }),
            "on the charger, held below full"
        );
    }
}
