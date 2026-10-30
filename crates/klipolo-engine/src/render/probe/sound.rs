//! What Sync reads off a file at import: where its sound starts against its picture, when the file was made,
//! and its sound's format (part of a recorder's signature, `DeviceSig`).

use std::path::Path;

use ffmpeg::format::context::Input;
use ffmpeg::format::stream::Stream;

/// A lead past this is a broken stream start, not a camera's: it is taken as 0. The model's bound, which every
/// reader of a lead clamps to (`Asset::lead_us`).
const MAX_LEAD_US: i64 = klipolo_model::project::MAX_AUDIO_LEAD_US;

/// The best audio stream's codec name (FFmpeg's, e.g. "aac", "pcm_s24le"), sample rate and channel count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFormat {
    pub codec: String,
    pub rate: u32,
    pub channels: u16,
}

/// Where a stream starts in µs, as its decoder counts from it (`stream_start`: its first timestamp, or 0
/// when the file doesn't state it); None when its time base is broken.
fn start_us(stream: &Stream) -> Option<i64> {
    let (t, tb) = (super::stream_start(stream), stream.time_base());
    (tb.denominator() > 0).then(|| {
        (i128::from(t) * 1_000_000 * i128::from(tb.numerator()) / i128::from(tb.denominator()))
            as i64
    })
}

/// The sound's start minus the picture's, in µs: where the sound cache's first frame plays against the picture's first,
/// since each decoder counts from its own stream's start (`stream_start`; the Mac's AVAssetReader too).
/// An encoder's priming that the edit list skips moves the stream's start with it, and priming it doesn't skip is
/// decoded into the cache, so the lead is right for the cache either way. A start the file doesn't state counts as 0,
/// as the decoders count it. 0 when a time base is broken, or past ±500 ms (logged).
pub(super) fn lead_us(picture: &Stream, sound: &Stream, path: &Path) -> i64 {
    let (Some(picture), Some(sound)) = (start_us(picture), start_us(sound)) else {
        return 0;
    };
    let lead = sound - picture;
    if lead.abs() > MAX_LEAD_US {
        tracing::warn!(
            lead_us = lead,
            file = %crate::log_name(path),
            "the sound's start is far from the picture's: taken as a broken start, not a lead"
        );
        return 0;
    }
    lead
}

/// The container's `creation_time`, in ms since 1970; None when it isn't stated, can't be read or is at or before 1970
/// (a camera whose clock was never set).
pub(super) fn created_ms(ctx: &Input) -> Option<i64> {
    ctx.metadata()
        .get("creation_time")
        .and_then(parse_utc_ms)
        .filter(|ms| *ms > 0)
}

pub(super) fn format(stream: &Stream) -> AudioFormat {
    let parameters = stream.parameters();
    // SAFETY: the parameters belong to the open input, which outlives this read; FFmpeg only reads them.
    let (rate, channels) = unsafe {
        let p = parameters.as_ptr();
        ((*p).sample_rate, (*p).ch_layout.nb_channels)
    };
    AudioFormat {
        codec: parameters.id().name().to_owned(),
        rate: u32::try_from(rate).unwrap_or(0),
        channels: u16::try_from(channels).unwrap_or(0),
    }
}

/// `n` ASCII digits at `at`.
fn digits(s: &str, at: usize, n: usize) -> Option<i64> {
    let part = s.get(at..at + n)?;
    part.bytes()
        .all(|c| c.is_ascii_digit())
        .then(|| part.parse().ok())?
}

/// `YYYY-MM-DD[T ]HH:MM:SS[.fff…][Z | ±HH[:]MM]`, as demuxers state `creation_time` (no zone is UTC), in ms since
/// 1970.
fn parse_utc_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    let separators = [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')];
    if separators.iter().any(|&(i, c)| b.get(i) != Some(&c))
        || !matches!(b.get(10), Some(b'T' | b't' | b' '))
    {
        return None;
    }
    let (year, month, day) = (digits(s, 0, 4)?, digits(s, 5, 2)?, digits(s, 8, 2)?);
    let (hour, minute, second) = (digits(s, 11, 2)?, digits(s, 14, 2)?, digits(s, 17, 2)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &s[19..];
    let mut ms = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let n = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        let kept = &fraction[..n.min(3)];
        ms = kept.parse::<i64>().ok()? * 10i64.pow(3 - kept.len() as u32);
        rest = &fraction[n..];
    }
    let offset_minutes = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let zone = rest[1..].replace(':', "");
            if zone.len() != 4 {
                return None;
            }
            sign * (digits(&zone, 0, 2)? * 60 + digits(&zone, 2, 2)?)
        }
    };
    let minutes = (days_from_civil(year, month, day) * 24 + hour) * 60 + minute - offset_minutes;
    Some(minutes * 60_000 + second * 1_000 + ms)
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::parse_utc_ms;

    #[test]
    fn creation_times_read_as_demuxers_state_them() {
        let at = Some(1_792_144_800_000);
        assert_eq!(parse_utc_ms("2026-10-16T10:00:00.000000Z"), at);
        assert_eq!(parse_utc_ms("2026-10-16 10:00:00"), at);
        assert_eq!(parse_utc_ms("2026-10-16T12:00:00+02:00"), at);
        assert_eq!(
            parse_utc_ms("2026-10-16T09:30:00.25-0030"),
            at.map(|t| t + 250)
        );
        assert_eq!(parse_utc_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc_ms("2000-02-29T00:00:00Z"), Some(951_782_400_000));
        for bad in [
            "",
            "2026-10-16",
            "2026-13-01T00:00:00Z",
            "2026-10-16T10:00:00 UTC",
            "+026-10-16T10:00:00Z",
        ] {
            assert_eq!(parse_utc_ms(bad), None, "{bad}");
        }
    }
}
