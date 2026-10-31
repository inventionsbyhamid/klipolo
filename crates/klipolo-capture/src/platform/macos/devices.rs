//! Cameras, microphones and the sound output, listed without capture (device inspection never
//! activates capture).
//! - **Cameras** come from AVFoundation's discovery session: names and states only, with no session and no input, so
//!   nothing opens and nothing asks. The system's default camera is first.
//! - **Microphones and the output** come from Core Audio's properties, never cpal's listing. A mic's id is its name,
//!   which the cpal microphone finds its device by (`mic::cpal_mic`). The system's default input is first, since
//!   preflight picks the first. Listing them costs a few property reads per device: `mics` runs once a second while a
//!   take is on a fallback mic.

use cidre::core_audio::{Device, PropElement, PropScope, PropSelector, System};
use cidre::{av, ns, objc};
use objc2::msg_send;
use objc2::runtime::{AnyObject, Sel};
use objc2_foundation::{NSBundle, NSNumber, ns_string};

use crate::backend::{DeviceInfo, Transport};
use crate::platform::common::mac_preflight::transport_of;

/// The input devices, the system's default first.
pub(super) fn mics() -> Vec<DeviceInfo> {
    let Ok(devices) = System::devices() else {
        return Vec::new();
    };
    let mut mics: Vec<DeviceInfo> = (devices.iter())
        .filter(|d| has_input(d) && !hidden(d))
        .filter_map(info)
        .collect();
    let default = System::default_input_device()
        .ok()
        .and_then(|d| name_of(&d));
    if let Some(at) = default.and_then(|name| mics.iter().position(|m| m.id == name)) {
        mics[..=at].rotate_right(1);
    }
    mics
}

/// Where the computer's sound goes now.
pub(super) fn output() -> Option<DeviceInfo> {
    System::default_output_device().ok().as_ref().and_then(info)
}

/// The cameras AVFoundation discovers (built-in, external, and Continuity when the app opts in), the default first.
/// A discovery the OS refuses (an exception) lists none rather than ending the process.
pub(super) fn cameras() -> Vec<DeviceInfo> {
    objc::ar_pool(|| {
        let mut types = vec![
            av::CaptureDeviceType::built_in_wide_angle_camera(),
            av::CaptureDeviceType::external(),
        ];
        if continuity_allowed() {
            types.push(av::CaptureDeviceType::continuity_camera());
        }
        let mut cameras: Vec<DeviceInfo> =
            discover(&types).map_or_else(Vec::new, |devices| devices.iter().map(camera).collect());
        let default =
            ns::try_catch(|| av::CaptureDevice::default_with_media(av::MediaType::video()));
        let default = default.ok().flatten().map(|d| d.unique_id().to_string());
        if let Some(at) = default.and_then(|id| cameras.iter().position(|c| c.id == id)) {
            cameras[..=at].rotate_right(1);
        }
        cameras
    })
}

fn discover(
    types: &[&av::CaptureDeviceType],
) -> Option<cidre::arc::R<ns::Array<av::CaptureDevice>>> {
    let types = ns::Array::from_slice(types);
    ns::try_catch(|| {
        av::CaptureDeviceDiscoverySession::with_device_types_media_and_pos(
            &types,
            Some(av::MediaType::video()),
            av::CaptureDevicePos::Unspecified,
        )
        .devices()
    })
    .ok()
}

/// Whether the app's `Info.plist` opts in to Continuity Camera (`NSCameraUseContinuityCameraDeviceType`).
/// Without it AVFoundation leaves those cameras out and logs a warning on every discovery.
fn continuity_allowed() -> bool {
    let key = ns_string!("NSCameraUseContinuityCameraDeviceType");
    (NSBundle::mainBundle().objectForInfoDictionaryKey(key))
        .and_then(|v| v.downcast_ref::<NSNumber>().map(|n| n.boolValue()))
        .unwrap_or(false)
}

fn camera(device: &av::CaptureDevice) -> DeviceInfo {
    let name = device.localized_name().to_string();
    // SAFETY: an AVCaptureDevice is an Objective-C object; this borrows the same object as `AnyObject`.
    let object = unsafe { &*(device as *const av::CaptureDevice).cast::<AnyObject>() };
    let built_in = av::CaptureDeviceType::built_in_wide_angle_camera();
    let transport = if device.is_continuity_camera() {
        Transport::Continuity
    } else if device.device_type().eq_ns_string(built_in) {
        // An Intel Mac's FaceTime camera reports USB, its internal bus.
        Transport::BuiltIn
    } else {
        // `transportType` is an `int32_t` holding the same four-character codes as Core Audio's.
        let code = answers(object, objc2::sel!(transportType))
            // SAFETY: `transportType` takes no argument and returns `int32_t`; the object answers it.
            .then(|| unsafe { msg_send![object, transportType] })
            .map_or(0, |code: i32| code as u32);
        transport_of(code, &name)
    };
    // SAFETY (both): no argument, a `BOOL` back, and the object answers the selector.
    let suspended =
        answers(object, objc2::sel!(isSuspended)) && unsafe { msg_send![object, isSuspended] };
    let in_use = answers(object, objc2::sel!(isInUseByAnotherApplication))
        && unsafe { msg_send![object, isInUseByAnotherApplication] };
    DeviceInfo {
        id: device.unique_id().to_string(),
        name,
        is_virtual: transport == Transport::Virtual,
        transport,
        in_use,
        suspended,
    }
}

/// Whether `object` answers `selector` (an older or newer AVFoundation may lack a property).
fn answers(object: &AnyObject, selector: Sel) -> bool {
    // SAFETY: `respondsToSelector:` takes a selector and returns `BOOL`, on every NSObject.
    unsafe { msg_send![object, respondsToSelector: selector] }
}

/// A Core Audio device as a mic or the output: its name (the mic's id) and transport.
fn info(device: &Device) -> Option<DeviceInfo> {
    let name = name_of(device)?;
    let code = device.transport_type().map_or(0, |t| t.0);
    let transport = transport_of(code, &name);
    Some(DeviceInfo {
        id: name.clone(),
        name,
        is_virtual: transport == Transport::Virtual,
        transport,
        in_use: false,
        suspended: false,
    })
}

/// `kAudioObjectPropertyName`, the name cpal reads (`kAudioDevicePropertyDeviceNameCFString` is the same property).
fn name_of(device: &Device) -> Option<String> {
    let name = device.name().ok()?.to_string();
    (!name.is_empty()).then_some(name)
}

/// Whether the device has input streams: a microphone (or a line in).
fn has_input(device: &Device) -> bool {
    let streams = PropSelector::DEVICE_STREAMS.addr(PropScope::INPUT, PropElement::MAIN);
    device.prop_size(&streams).is_ok_and(|bytes| bytes > 0)
}

/// A device its driver hides from device lists.
fn hidden(device: &Device) -> bool {
    device
        .bool_prop(&PropSelector::DEVICE_IS_HIDDEN.global_addr())
        .unwrap_or(false)
}

/// How long the mic named `name` takes to deliver what it hears, as Core Audio reports it (no capture: property reads):
/// the input's device latency, safety offset and largest stream latency over its nominal rate (`mic::latency_ns`, capped
/// at 0.5 s). cpal's block stamp is the buffer's `mHostTime`, the time the HAL had it, so a sound is stamped this much
/// late (the A/V check).
pub fn input_latency(name: &str) -> Option<std::time::Duration> {
    let devices = System::devices().ok()?;
    let device = devices
        .into_iter()
        .find(|d| has_input(d) && name_of(d).as_deref() == Some(name))?;
    let frames = |selector: PropSelector| {
        device
            .prop::<u32>(&selector.addr(PropScope::INPUT, PropElement::MAIN))
            .unwrap_or(0)
    };
    let streams = (device.streams().ok()?.into_iter())
        .filter(|s| s.direction().is_ok_and(|d| d == 1))
        .filter_map(|s| s.latency().ok())
        .max()
        .unwrap_or(0);
    let rate = device.nominal_sample_rate().ok().filter(|r| *r > 0.0)?;
    Some(std::time::Duration::from_nanos(crate::mic::latency_ns(
        frames(PropSelector::DEVICE_LATENCY),
        frames(PropSelector::DEVICE_SAFETY_OFFSET),
        streams,
        rate,
    )))
}

/// The mic named `name`'s nominal and actual sample rates (Core Audio's properties; the actual one is measured while
/// the device runs): what a drift line should find.
pub fn input_rates(name: &str) -> Option<(f64, f64)> {
    let devices = System::devices().ok()?;
    let device = devices
        .into_iter()
        .find(|d| has_input(d) && name_of(d).as_deref() == Some(name))?;
    Some((
        device.nominal_sample_rate().ok()?,
        device.actual_sample_rate().ok()?,
    ))
}
