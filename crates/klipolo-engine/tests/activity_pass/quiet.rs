//! The public quiet-sound accessor: spans of a file's sound whose every channel stays below a
//! level, read from the sound cache's 100 ms energy windows.

use std::io::Write;
use std::path::Path;

use klipolo_engine::audio::pcm::{PcmKey, PcmStore, quiet_spans};
use klipolo_model::time::Us;

const RATE: usize = 48_000;
const S: Us = 1_000_000;

/// 10 s of 16-bit stereo: 0–2 s a loud tone on both channels, 2–5 s silence, 5–7 s the right channel alone, 7–8.5 s a
/// tone at about −53 dBFS on both, 8.5–10 s loud again.
fn write_wav(path: &Path) {
    let frames = 10 * RATE;
    let data = (frames * 4) as u32;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    for part in [
        &b"RIFF"[..],
        &(36 + data).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &2u16.to_le_bytes(),
        &(RATE as u32).to_le_bytes(),
        &(RATE as u32 * 4).to_le_bytes(),
        &4u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
    ] {
        out.write_all(part).unwrap();
    }
    for n in 0..frames {
        let t = n as f64 / RATE as f64;
        let tone = |amp: f64, hz: f64| amp * (2.0 * std::f64::consts::PI * hz * t).sin();
        let (l, r) = match t {
            t if t < 2.0 => (tone(0.5, 440.0), tone(0.25, 220.0)),
            t if t < 5.0 => (0.0, 0.0),
            t if t < 7.0 => (0.0, tone(0.25, 220.0)),
            t if t < 8.5 => (tone(0.003, 440.0), tone(0.003, 440.0)),
            _ => (tone(0.5, 440.0), tone(0.25, 220.0)),
        };
        for x in [l, r] {
            out.write_all(&((x * 32767.0).round() as i16).to_le_bytes())
                .unwrap();
        }
    }
}

fn near(got: (Us, Us), want: (Us, Us)) -> bool {
    (got.0 - want.0).abs() <= 100_000 && (got.1 - want.1).abs() <= 100_000
}

#[test]
fn quiet_spans_come_from_the_energy_windows() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("voice.wav");
    write_wav(&wav);
    let path = wav.display().to_string();
    let store = PcmStore::new(dir.path().join("cache"));
    assert_eq!(
        quiet_spans(&store, &path, -40.0, S),
        None,
        "none until the sound cache has the file"
    );
    store.build(&PcmKey { path: path.clone() }).unwrap();
    let spans = quiet_spans(&store, &path, -40.0, S).unwrap();
    assert_eq!(spans.len(), 2, "{spans:?}");
    // The right channel alone at 5–7 s keeps it loud: every channel must be quiet.
    assert!(near(spans[0], (2 * S, 5 * S)), "{spans:?}");
    assert!(near(spans[1], (7 * S, 8_500_000)), "{spans:?}");
    // Shorter than `min_us` is left out; a higher threshold doesn't make the right channel's tone quiet.
    let long = quiet_spans(&store, &path, -40.0, 2 * S).unwrap();
    assert_eq!(long.len(), 1, "{long:?}");
    assert!(near(long[0], (2 * S, 5 * S)), "{long:?}");
    let low = quiet_spans(&store, &path, -60.0, S).unwrap();
    assert_eq!(low.len(), 1, "the −53 dBFS tone isn't below −60: {low:?}");
}
