//! CoreAudio route listeners and the default output's latency (moved from `klipolo-engine/src/audio/route.rs` and
//! `audio/stream.rs`'s `hardware_latency`; patterns from Cap `crates/audio/src/latency.rs`, not vendored).

use std::ffi::c_void;
use std::time::Duration;

use cidre::core_audio::hardware::System;
use cidre::core_audio::{
    Device, DeviceTransportType, Obj, PropAddr, PropElement, PropScope, PropSelector,
};
use cidre::os;

/// The mixer's rate, assumed when the device doesn't report its own.
const RATE: u32 = 48_000;

extern "C-unwind" fn changed(
    _obj: Obj,
    _count: u32,
    _addresses: *const PropAddr,
    _data: *mut c_void,
) -> os::Status {
    crate::audio_route::changed();
    os::Status::NO_ERR
}

fn device_props() -> [PropAddr; 4] {
    [
        PropSelector::DEVICE_LATENCY.addr(PropScope::OUTPUT, PropElement::MAIN),
        PropSelector::DEVICE_SAFETY_OFFSET.addr(PropScope::OUTPUT, PropElement::MAIN),
        PropSelector::DEVICE_BUF_FRAME_SIZE.global_addr(),
        PropSelector::DEVICE_NOMINAL_SAMPLE_RATE.global_addr(),
    ]
}

/// The listeners of one output; removed on drop.
pub struct RouteWatch {
    /// Identifies this watch's registrations (never dereferenced).
    tag: Box<u8>,
    device: Option<Device>,
}

impl RouteWatch {
    pub fn new() -> Self {
        let mut watch = Self {
            tag: Box::new(0),
            device: None,
        };
        let tag = watch.tag_ptr();
        let _ = System::OBJ.add_prop_listener(
            &PropSelector::HW_DEFAULT_OUTPUT_DEVICE.global_addr(),
            changed,
            tag,
        );
        watch.follow();
        watch
    }

    fn tag_ptr(&mut self) -> *mut c_void {
        (&mut *self.tag as *mut u8).cast()
    }

    /// Moves the device listeners to the current default output.
    pub fn follow(&mut self) {
        let current = System::default_output_device().ok();
        if current.as_ref().map(|d| d.0.0) == self.device.as_ref().map(|d| d.0.0) {
            return;
        }
        self.unwatch_device();
        let tag = self.tag_ptr();
        if let Some(device) = &current {
            for addr in device_props() {
                let _ = device.add_prop_listener(&addr, changed, tag);
            }
        }
        self.device = current;
    }

    fn unwatch_device(&mut self) {
        let tag = self.tag_ptr();
        if let Some(device) = self.device.take() {
            for addr in device_props() {
                let _ = device.remove_prop_listener(&addr, changed, tag);
            }
        }
    }
}

impl Default for RouteWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RouteWatch {
    fn drop(&mut self) {
        self.unwatch_device();
        let tag = self.tag_ptr();
        let _ = System::OBJ.remove_prop_listener(
            &PropSelector::HW_DEFAULT_OUTPUT_DEVICE.global_addr(),
            changed,
            tag,
        );
    }
}

/// How long after the callback's buffer is due the device actually plays it: device latency, safety offset and
/// stream latency from CoreAudio, at least 120 ms over Bluetooth and 1.8 s over AirPlay (Cap's floors). Read from the
/// default output device (the one cpal's default unit follows) at start and after every route or latency change.
pub fn hardware_latency() -> Duration {
    let Ok(device) = System::default_output_device() else {
        return Duration::ZERO;
    };
    let output = |selector: PropSelector| -> u32 {
        device
            .prop(&selector.addr(PropScope::OUTPUT, PropElement::MAIN))
            .unwrap_or(0)
    };
    let stream = device
        .streams()
        .unwrap_or_default()
        .iter()
        .filter(|s| s.direction().is_ok_and(|d| d == 0))
        .filter_map(|s| s.latency().ok())
        .max()
        .unwrap_or(0);
    let frames =
        output(PropSelector::DEVICE_LATENCY) + output(PropSelector::DEVICE_SAFETY_OFFSET) + stream;
    let rate = device
        .nominal_sample_rate()
        .ok()
        .filter(|r| *r > 0.0)
        .unwrap_or(f64::from(RATE));
    let mut secs = f64::from(frames) / rate;
    let transport = device
        .transport_type()
        .unwrap_or(DeviceTransportType::UNKNOWN);
    if transport == DeviceTransportType::BLUETOOTH || transport == DeviceTransportType::BLUETOOTH_LE
    {
        secs = secs.max(0.12);
    } else if transport == DeviceTransportType::AIR_PLAY {
        secs = secs.max(1.8);
    }
    Duration::from_secs_f64(secs.clamp(0.0, 3.0))
}
