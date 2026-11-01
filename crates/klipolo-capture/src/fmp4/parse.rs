//! Parsing the boxes the walk reads into memory: a head `trak` and a `moof`.

use super::boxes::{Rd, child, children, path};
use super::read::TrackInfo;
use super::{FragIndex, NON_SYNC, SampleIndex};
use crate::media::{Codec, StreamInfo};

/// The head track's id and its `trex` defaults (zeros without a head).
#[derive(Default)]
pub(crate) struct Track {
    pub(crate) id: Option<u32>,
    pub(crate) duration: u32,
    pub(crate) size: u32,
    pub(crate) flags: u32,
}

/// A video `trak`'s description and track id; None for any other track.
pub(crate) fn trak(b: &[u8]) -> Option<(TrackInfo, u32)> {
    let mut r = Rd::new(child(b, b"tkhd")?);
    let (v, _) = r.version_flags()?;
    r.skip(if v == 1 { 16 } else { 8 })?;
    let id = r.u32()?;
    let mdia = child(b, b"mdia")?;
    let mut r = Rd::new(child(mdia, b"hdlr")?);
    r.version_flags()?;
    r.skip(4)?;
    if r.take(4)? != b"vide" {
        return None;
    }
    let mut r = Rd::new(child(mdia, b"mdhd")?);
    let (v, _) = r.version_flags()?;
    r.skip(if v == 1 { 16 } else { 8 })?;
    let timescale = r.u32()?;
    let mut r = Rd::new(path(mdia, &[b"minf", b"stbl", b"stsd"])?);
    r.version_flags()?;
    r.u32()?;
    let (kind, entry) = children(r.rest()).next()?;
    let (codec, config) = match &kind {
        b"avc1" | b"avc3" => (Codec::H264, b"avcC"),
        b"hvc1" | b"hev1" => (Codec::Hevc, b"hvcC"),
        _ => return None,
    };
    let mut r = Rd::new(entry);
    r.skip(24)?;
    let (width, height) = (r.u16()?, r.u16()?);
    r.skip(50)?;
    let config = child(r.rest(), config)?.to_vec();
    let info = StreamInfo {
        codec,
        config,
        width: width.into(),
        height: height.into(),
    };
    Some((TrackInfo { info, timescale }, id))
}

/// The track's samples in a `moof` body: `tfhd` (with its defaults), `tfdt`, and every `trun`, whose samples must follow
/// each other. More than `max_samples` in all is damage.
pub(crate) fn moof_index(
    moof: &[u8],
    moof_at: u64,
    track: &Track,
    next_dts: u64,
    max_samples: u64,
) -> Option<FragIndex> {
    // The head track's `traf` (without a head, the first).
    let (traf, flags, mut r) =
        children(moof)
            .filter(|(k, _)| k == b"traf")
            .find_map(|(_, traf)| {
                let mut r = Rd::new(child(traf, b"tfhd")?);
                let (_, flags) = r.version_flags()?;
                let id = r.u32()?;
                track.id.is_none_or(|t| t == id).then_some((traf, flags, r))
            })?;
    let base = if flags & 0x01 != 0 { r.u64()? } else { moof_at };
    if flags & 0x02 != 0 {
        r.u32()?;
    }
    let dur = if flags & 0x08 != 0 {
        r.u32()?
    } else {
        track.duration
    };
    let size = if flags & 0x10 != 0 {
        r.u32()?
    } else {
        track.size
    };
    let sflags = if flags & 0x20 != 0 {
        r.u32()?
    } else {
        track.flags
    };
    let first_dts = match child(traf, b"tfdt") {
        Some(t) => {
            let mut r = Rd::new(t);
            match r.version_flags()?.0 {
                1 => r.u64()?,
                _ => u64::from(r.u32()?),
            }
        }
        None => next_dts,
    };
    let (mut samples, mut data_at, mut end) = (Vec::new(), None, base);
    for (_, trun) in children(traf).filter(|(k, _)| k == b"trun") {
        let mut r = Rd::new(trun);
        let (_, f) = r.version_flags()?;
        let count = r.u32()? as usize;
        let at = match f & 0x01 {
            0 => end,
            _ => base.checked_add_signed(i64::from(r.u32()? as i32))?,
        };
        match data_at {
            None => data_at = Some(at),
            Some(_) if at != end => return None,
            Some(_) => {}
        }
        end = at;
        let first = if f & 0x04 != 0 { Some(r.u32()?) } else { None };
        let per = [0x100, 0x200, 0x400, 0x800]
            .iter()
            .filter(|b| f & **b != 0)
            .count()
            * 4;
        // A count the box can't hold, or past `max_samples` (with every field defaulted, the box bounds nothing), is
        // damage, before anything is reserved.
        if count.checked_mul(per)? > r.rest().len() || (samples.len() + count) as u64 > max_samples
        {
            return None;
        }
        samples.reserve(count);
        for i in 0..count {
            let d = if f & 0x100 != 0 { r.u32()? } else { dur };
            let s = if f & 0x200 != 0 { r.u32()? } else { size };
            let fl = match (f & 0x400 != 0, i, first) {
                (true, _, _) => r.u32()?,
                (false, 0, Some(first)) => first,
                _ => sflags,
            };
            if f & 0x800 != 0 {
                r.u32()?;
            }
            samples.push(SampleIndex {
                size: s,
                duration: d,
                key: fl & NON_SYNC == 0,
            });
            end += u64::from(s);
        }
    }
    (!samples.is_empty()).then_some(FragIndex {
        moof_at,
        data_at: data_at?,
        first_dts,
        samples,
    })
}
