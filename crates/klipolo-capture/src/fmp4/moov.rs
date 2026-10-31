//! The track's `moov`, shared by the head (empty sample tables and `mvex`) and the remux tail (the whole table).

use super::boxes::{be16, be32, be64, bx, full};
use crate::media::{Codec, StreamInfo};

/// The one track's id.
pub(crate) const TRACK_ID: u32 = 1;

/// The track's `moov`: the head's (no samples in `stbl`'s `tables`, and `mvex`) or the remux tail's (the whole sample
/// table, no `mvex`). `duration` is in `timescale`.
pub(crate) fn moov(
    out: &mut Vec<u8>,
    info: &StreamInfo,
    timescale: u32,
    duration: u64,
    fragmented: bool,
    tables: impl FnOnce(&mut Vec<u8>),
) {
    let ms = ((u128::from(duration) * 1000 + u128::from(timescale / 2))
        / u128::from(timescale.max(1))) as u64;
    bx(out, b"moov", |o| {
        let v1 = ms > u64::from(u32::MAX);
        full(o, b"mvhd", u8::from(v1), 0, |o| {
            times(o, v1, 1000);
            dur(o, v1, ms);
            be32(o, 0x0001_0000);
            be16(o, 0x0100);
            o.extend_from_slice(&[0; 10]);
            matrix(o);
            o.extend_from_slice(&[0; 24]);
            be32(o, TRACK_ID + 1);
        });
        bx(o, b"trak", |o| {
            // Enabled and in the movie.
            full(o, b"tkhd", u8::from(v1), 3, |o| {
                times(o, v1, TRACK_ID);
                be32(o, 0);
                dur(o, v1, ms);
                o.extend_from_slice(&[0; 16]);
                matrix(o);
                be32(o, info.width.min(0xffff) << 16);
                be32(o, info.height.min(0xffff) << 16);
            });
            bx(o, b"mdia", |o| {
                let v1 = duration > u64::from(u32::MAX);
                full(o, b"mdhd", u8::from(v1), 0, |o| {
                    times(o, v1, timescale);
                    dur(o, v1, duration);
                    be16(o, 0x55c4); // "und"
                    be16(o, 0);
                });
                full(o, b"hdlr", 0, 0, |o| {
                    be32(o, 0);
                    o.extend_from_slice(b"vide");
                    o.extend_from_slice(&[0; 12]);
                    o.extend_from_slice(b"VideoHandler\0");
                });
                bx(o, b"minf", |o| {
                    full(o, b"vmhd", 0, 1, |o| o.extend_from_slice(&[0; 8]));
                    bx(o, b"dinf", |o| {
                        full(o, b"dref", 0, 0, |o| {
                            be32(o, 1);
                            full(o, b"url ", 0, 1, |_| {});
                        })
                    });
                    bx(o, b"stbl", |o| {
                        stsd(o, info);
                        tables(o);
                    });
                });
            });
        });
        if fragmented {
            bx(o, b"mvex", |o| {
                full(o, b"trex", 0, 0, |o| {
                    for v in [TRACK_ID, 1, 0, 0, 0] {
                        be32(o, v);
                    }
                })
            });
        }
    });
}

/// mvhd's, tkhd's and mdhd's leading fields: creation and modification times (0), then `id` (a timescale, or tkhd's
/// track id).
fn times(o: &mut Vec<u8>, v1: bool, id: u32) {
    if v1 {
        be64(o, 0);
        be64(o, 0);
    } else {
        be32(o, 0);
        be32(o, 0);
    }
    be32(o, id);
}

/// A duration: 64 bits in a version-1 box, else 32.
fn dur(o: &mut Vec<u8>, v1: bool, d: u64) {
    if v1 { be64(o, d) } else { be32(o, d as u32) }
}

fn matrix(o: &mut Vec<u8>) {
    for v in [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000u32] {
        be32(o, v);
    }
}

/// One `avc1` (with `avcC`) or `hvc1` (with `hvcC`) sample entry from the encoder's config record.
fn stsd(o: &mut Vec<u8>, info: &StreamInfo) {
    let (entry, config) = match info.codec {
        Codec::H264 => (b"avc1", b"avcC"),
        Codec::Hevc => (b"hvc1", b"hvcC"),
    };
    full(o, b"stsd", 0, 0, |o| {
        be32(o, 1);
        bx(o, entry, |o| {
            o.extend_from_slice(&[0; 6]);
            be16(o, 1); // data reference index
            o.extend_from_slice(&[0; 16]);
            be16(o, info.width.min(0xffff) as u16);
            be16(o, info.height.min(0xffff) as u16);
            be32(o, 0x0048_0000); // 72 dpi
            be32(o, 0x0048_0000);
            be32(o, 0);
            be16(o, 1); // frame count
            o.extend_from_slice(&[0; 32]);
            be16(o, 0x0018);
            be16(o, 0xffff);
            bx(o, config, |o| o.extend_from_slice(&info.config));
        });
    });
}
