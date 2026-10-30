//! The sound track. The whole mix is rendered in one pass, on its own thread while the pictures render (it is
//! cheap next to them), encoded once and shared by every format.
//!
//! AAC's priming is handled without an edit list. The encoder gets the mix from sample `priming` on, so the decoded
//! sample n is the timeline's sample n; the first ~44 ms are the encoder's warm-up. The encoded track's true peak is
//! measured, and if it is over −1 dBTP the track is rendered again under a ceiling lowered by the overshoot.
//!
//! The worker waits for it without blocking (`sound_wait.rs`), telling its parent every few seconds while it
//! advances, so a long sound after the pictures never looks like a stall.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ffmpeg::format::sample::{Sample, Type};
use ffmpeg::{codec, format, frame, media};

use super::audio::ExportAudio;
use super::job::PartStatus;
use super::protocol::ErrorCode;
use super::quality::AudioCodec;
use super::shared::{Control, JobHandle, WorkerError, checksum, ff_error, internal, io_error};
pub use super::sound_wait::{Sound, SoundProgress, wait};
use crate::audio::mix::RATE;
use crate::vendor::cap::enc_ffmpeg::aac::AACEncoder;
use crate::vendor::cap::media_info::AudioInfo;

/// Samples per block handed to the encoder.
const BLOCK: usize = 4_096;
/// Every export's true peak (uploads; platforms re-encode).
const LIMIT_DBTP: f64 = -1.0;
/// Renders at most this many times while the encoded peak is over the limit.
const TRIES: usize = 3;

/// Renders the job's sound track on its own thread. Ok(false): stopped by a pause (nothing kept).
pub fn spawn(
    audio: ExportAudio,
    job: JobHandle,
    control: Arc<Control>,
) -> Result<Sound, WorkerError> {
    let progress = Arc::new(SoundProgress::default());
    let shared = progress.clone();
    let thread = std::thread::Builder::new()
        .name("klipolo-export-sound".into())
        .spawn(move || {
            // The pictures are the critical path: the sound (needed only to join) yields to them.
            crate::qos::set_current(crate::qos::Qos::Utility);
            render(&audio, &job, &control, &shared)
        })
        .map_err(internal)?;
    Ok(Sound { thread, progress })
}

fn render(
    audio: &ExportAudio,
    job: &JobHandle,
    control: &Control,
    progress: &SoundProgress,
) -> Result<bool, WorkerError> {
    let (track, path, samples, bitrate) = job.read(|j| {
        let samples = j.total_frames * u64::from(RATE) / u64::from(j.fps);
        (
            j.audio.clone(),
            j.path(&j.audio.file),
            samples,
            j.audio_bitrate,
        )
    });
    let started = std::time::Instant::now();
    let (mut ceiling, mut peak) = (track.ceiling_dbtp, None);
    for _ in 0..TRIES {
        progress.start_pass(samples);
        let encoded = encode(
            audio,
            track.codec,
            &path,
            (samples, ceiling, bitrate),
            control,
            progress,
        )?;
        if !encoded {
            let _ = std::fs::remove_file(&path);
            return Ok(false);
        }
        if track.codec != AudioCodec::Aac192 {
            break;
        }
        progress.start_pass(samples);
        peak = peak_of(&path, progress, control);
        if !control.running() {
            let _ = std::fs::remove_file(&path);
            return Ok(false);
        }
        match peak {
            Some(p) if p > LIMIT_DBTP => {
                tracing::info!(
                    peak = p,
                    ceiling,
                    "encoded sound over the limit: rendering lower"
                );
                ceiling -= (p - LIMIT_DBTP) as f32 + 0.2;
            }
            _ => break,
        }
    }
    let file = std::fs::File::open(&path).map_err(|e| io_error(e, "the sound track"))?;
    file.sync_all()
        .map_err(|e| io_error(e, "saving the sound track"))?;
    drop(file);
    let bytes = std::fs::metadata(&path)
        .map_err(|e| io_error(e, "the sound track"))?
        .len();
    let crc = checksum(&path).map_err(|e| io_error(e, "the sound track"))?;
    eprintln!(
        "export sound: {:.1} s of audio in {:.2} s; ceiling {ceiling:.1} dBTP, encoded true peak {peak:?}",
        samples as f64 / f64::from(RATE),
        started.elapsed().as_secs_f64()
    );
    job.update(|j| {
        j.audio.status = PartStatus::Done;
        j.audio.bytes = bytes;
        j.audio.checksum = crc;
        j.audio.ceiling_dbtp = ceiling;
        j.audio.true_peak_db = peak;
    })?;
    Ok(true)
}

/// Encodes `samples` of the mix limited at `ceiling` (AAC at `bitrate`) into `path`. False when a pause stopped it.
fn encode(
    audio: &ExportAudio,
    codec: AudioCodec,
    path: &Path,
    (samples, ceiling, bitrate): (u64, f32, u32),
    control: &Control,
    progress: &SoundProgress,
) -> Result<bool, WorkerError> {
    let container = match codec {
        AudioCodec::Aac192 => "mp4",
        AudioCodec::Pcm24 => "mov",
    };
    let mut output =
        format::output_as(path, container).map_err(|e| ff_error(e, "creating the sound track"))?;
    let info = AudioInfo::new_raw(Sample::F32(Type::Packed), RATE, 2);
    let encoder = match codec {
        AudioCodec::Aac192 => AACEncoder::init(info, bitrate as usize, &mut output),
        AudioCodec::Pcm24 => AACEncoder::init_pcm24(info, &mut output),
    };
    let mut encoder = encoder.map_err(|e| WorkerError {
        code: ErrorCode::Encode,
        message: format!("the sound encoder couldn't start: {e}"),
        retryable: true,
    })?;
    // The stored timestamps don't matter (the join re-times the track from 0): no edit list to read back.
    let mut options = ffmpeg::Dictionary::new();
    options.set("use_editlist", "0");
    output
        .write_header_with(options)
        .map_err(|e| ff_error(e, "starting the sound track"))?;
    let priming = (encoder.initial_padding() as u64).min(samples);
    let mut stream = audio.stream(ceiling);
    let mut block = vec![0f32; BLOCK * 2];
    let mut skip = priming;
    while skip > 0 {
        let n = skip.min(BLOCK as u64) as usize;
        stream.next(&mut block[..n * 2]);
        skip -= n as u64;
    }
    let (mut at, mut pts) = (priming, 0i64);
    while at < samples {
        if !control.running() {
            return Ok(false);
        }
        let n = (samples - at).min(BLOCK as u64) as usize;
        stream.next(&mut block[..n * 2]);
        let mut frame =
            frame::Audio::new(Sample::F32(Type::Packed), n, ffmpeg::ChannelLayout::STEREO);
        frame.set_rate(RATE);
        frame.set_pts(Some(pts));
        frame.data_mut(0)[..n * 8].copy_from_slice(bytemuck::cast_slice(&block[..n * 2]));
        encoder
            .send_frame(frame, Duration::MAX, &mut output)
            .map_err(|e| ff_error(e, "encoding the sound"))?;
        at += n as u64;
        pts += n as i64;
        progress.advance(n as u64);
    }
    encoder
        .flush(&mut output)
        .map_err(|e| ff_error(e, "finishing the sound"))?;
    output
        .write_trailer()
        .map_err(|e| ff_error(e, "finishing the sound track"))?;
    Ok(true)
}

/// The decoded track's true peak (dBTP, EBU R128's 4× oversampled meter); None for silence or an unreadable file.
pub fn true_peak(path: &Path) -> Option<f64> {
    peak_of(path, &SoundProgress::default(), &Control::default())
}

/// `true_peak`, counting decoded samples into `progress` and giving up (None) when `control` stops.
fn peak_of(path: &Path, progress: &SoundProgress, control: &Control) -> Option<f64> {
    let mut input = crate::media::open::input(path).ok()?;
    let stream = input.streams().best(media::Type::Audio)?;
    let index = stream.index();
    let mut decoder = codec::context::Context::from_parameters(stream.parameters())
        .ok()?
        .decoder()
        .audio()
        .ok()?;
    let channels = u32::from(decoder.channels());
    let mut meter =
        ebur128::EbuR128::new(channels, decoder.rate(), ebur128::Mode::TRUE_PEAK).ok()?;
    let mut frame = frame::Audio::empty();
    let mut drain = |decoder: &mut ffmpeg::decoder::Audio, meter: &mut ebur128::EbuR128| {
        while decoder.receive_frame(&mut frame).is_ok() {
            progress.advance(frame.samples() as u64);
            if frame.format() != Sample::F32(Type::Planar) {
                continue;
            }
            let planes: Vec<&[f32]> = (0..frame.planes()).map(|p| frame.plane::<f32>(p)).collect();
            let _ = meter.add_frames_planar_f32(&planes);
        }
    };
    for (s, packet) in input.packets() {
        if !control.running() {
            return None;
        }
        if s.index() == index && decoder.send_packet(&packet).is_ok() {
            drain(&mut decoder, &mut meter);
        }
    }
    let _ = decoder.send_eof();
    drain(&mut decoder, &mut meter);
    let peak = (0..channels)
        .filter_map(|c| meter.true_peak(c).ok())
        .fold(0f64, f64::max);
    (peak > 0.0).then(|| 20.0 * peak.log10())
}
