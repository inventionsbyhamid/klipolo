//! The microphone. On macOS and Windows it records through Cap's cpal fork (`CpalMic`: CoreAudio, WASAPI); Linux
//! records it as a PipeWire stream instead, because cpal's ALSA stream stamps with `MonotonicRaw`, not the host
//! clock, and would drift from every other source.
//!
//! **Consent.** Microphones are never listed through cpal: its listing iterator filters every device by opening it
//! (an ALSA PCM for capture, a WASAPI `IAudioClient`), which is device inspection that activates capture. Each OS lists
//! them from its registry, `pick_mic` picks one, and `CpalMic` finds
//! that device by name only when the person's Record press starts it (`tests/consent_scan.rs` pins this).
//!
//! **Real time.** The device's callback only converts, folds and stamps its block into bounded lock-free rings
//! (`ring`): no allocation, lock or I/O on the device's thread. A pump thread turns them into `PcmBlock`s.
//!
//! **Channels.** A mic records one channel or two, as the device gives them; a device with more (an audio interface)
//! records its first two inputs, where a mic is plugged in (the CAF header has no `chan` chunk, so more
//! than two would have no layout).

#[cfg(all(feature = "real-capture", any(target_os = "macos", windows)))]
mod cpal_mic;
pub mod ring;

#[cfg(all(feature = "real-capture", any(target_os = "macos", windows)))]
pub use cpal_mic::CpalMic;
pub use ring::{Flow, MicFeed, MicPump, Shared, handoff};

use klipolo_model::coded::CodedError;

use crate::devices::{DeviceInfo, Transport};
use crate::english::msg;

/// The most input latency a mic's stamps are moved back by (Cap's bound).
pub const MAX_INPUT_LATENCY_NS: u64 = 500_000_000;

/// How long a mic takes to deliver what it hears, from Core Audio's input properties in frames (the device's latency,
/// its safety offset and the largest input stream's latency) at `rate` Hz: at most `MAX_INPUT_LATENCY_NS`, and 0 at a
/// rate that isn't known. cpal stamps a block when the HAL had it (`mHostTime`), so the stamps move back by this.
/// The safety offset is in the sum, as Cap's design has it; whether Core Audio's stamp already sits after it is
/// for a clap test on a real Mac to show.
pub fn latency_ns(device: u32, safety: u32, stream: u32, rate: f64) -> u64 {
    if !(rate.is_finite() && rate > 0.0) {
        return 0;
    }
    let frames = u64::from(device) + u64::from(safety) + u64::from(stream);
    let ns = frames as f64 * 1e9 / rate;
    (ns as u64).min(MAX_INPUT_LATENCY_NS)
}

/// The mic a take records by default, as its id from `Devices::mics` (what
/// `AudioSpec::Mic` carries), with a note when preflight should say why:
/// - the person's `wanted` mic (an id) while it is connected;
/// - else the system default, listed first; but when the sound plays through a Bluetooth headset whose own mic that
///   is, the built-in mic, else a USB one, with the `bluetoothMic` note: recording through a headset's mic drops it to
///   its low-quality call profile (HFP), for the voice and for the sound the person hears. With no other mic, the
///   headset's is better than none.
///
/// A `wanted` mic that isn't connected is never replaced silently: the note is then `micMissing`, naming the mic picked
/// instead.
pub fn pick_mic(
    mics: &[DeviceInfo],
    output: Option<&DeviceInfo>,
    wanted: Option<&str>,
) -> (Option<String>, Option<CodedError>) {
    if let Some(chosen) = wanted.and_then(|w| mics.iter().find(|m| m.id == w)) {
        return (Some(chosen.id.clone()), None);
    }
    let Some(default) = mics.first() else {
        return (None, None);
    };
    let mut picked = (default, None);
    if output.is_some_and(|out| same_headset(default, out)) {
        let other = |t: Transport| mics.iter().find(|m| m.transport == t);
        if let Some(m) = other(Transport::BuiltIn).or_else(|| other(Transport::Usb)) {
            let note = msg!("capture.preflight.bluetoothMic", fallback = m.name.as_str());
            picked = (m, Some(note));
        }
    }
    let (mic, note) = picked;
    let note = match wanted {
        Some(_) => Some(msg!(
            "capture.preflight.micMissing",
            fallback = mic.name.as_str()
        )),
        None => note,
    };
    (Some(mic.id.clone()), note)
}

/// Whether `mic` belongs to the Bluetooth headset `output` plays through. macOS lists a headset as one device (one id
/// and name); Windows lists its mic and its output as two endpoints with different names ("Headset Microphone (…)",
/// "Headphones (…)"), so any Bluetooth mic counts while the sound goes to Bluetooth.
fn same_headset(mic: &DeviceInfo, output: &DeviceInfo) -> bool {
    output.transport == Transport::Bluetooth
        && (mic.transport == Transport::Bluetooth || mic.id == output.id || mic.name == output.name)
}

/// The person's own mic, back after a fallback, must stay listed this long before the take switches back to it, so
/// a flapping USB mic never churns runs. A mic back before any fallback restarts at once (a blip).
pub const PRESENT_NS: u64 = 3_000_000_000;
/// The take switches back from a fallback to the person's own mic at most this often.
pub const SWITCH_BACK_EVERY_NS: u64 = 30_000_000_000;

/// What a look for a lost mic saw (`Presence::look`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    Gone,
    /// Listed again, not yet for `PRESENT_NS`.
    Settling,
    /// Listed for `PRESENT_NS` straight: the take can record on it again.
    Present,
    /// It was listed, and now it's gone again: a flapping cable or port.
    Flapped,
}

/// The looks for a lost mic, about once a second, debounced: it is `Present` only once it has been listed
/// for `PRESENT_NS` straight, and going again resets that. Pure: every time is an argument (ns on any one clock).
#[derive(Clone, Copy, Debug, Default)]
pub struct Presence {
    since: Option<u64>,
}

impl Presence {
    pub fn look(&mut self, listed: bool, now_ns: u64) -> Seen {
        match (listed, self.since) {
            (false, Some(_)) => {
                self.since = None;
                Seen::Flapped
            }
            (false, None) => Seen::Gone,
            (true, None) => {
                self.since = Some(now_ns);
                Seen::Settling
            }
            (true, Some(t)) if now_ns.saturating_sub(t) >= PRESENT_NS => Seen::Present,
            (true, Some(_)) => Seen::Settling,
        }
    }

    /// Present again needs another `PRESENT_NS` of being listed.
    pub fn reset(&mut self) {
        self.since = None;
    }
}

/// The channels a mic records from a device with `device` channels: one or two (the module's "Channels").
pub fn mic_channels(device: u16) -> u16 {
    device.min(2)
}

/// A 16-bit sample in [-1, 1).
pub fn i16_to_f32(s: i16) -> f32 {
    f32::from(s) * (1.0 / 32_768.0)
}

/// A 32-bit sample in [-1, 1) (24-bit devices fill the top 24 bits, which an `f32` keeps exactly).
pub fn i32_to_f32(s: i32) -> f32 {
    s as f32 * (1.0 / 2_147_483_648.0)
}
