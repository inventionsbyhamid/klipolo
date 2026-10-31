//! A resumed take's writer: rebuilt from the cursor file the crashed worker left, so the file goes on
//! as one. No second header (unless the file has none: a crash before its first durable tick), the shape
//! store from its `S` lines (ids continue), the visibility and shape it shows, and nothing older than
//! each kind's last line (`CursorTrack::last_times`).

use klipolo_cursor::{CursorTrack, Limits};

use super::state::Kept;
use super::straight::Straight;
use super::{CursorWriter, Reorder, ShapeStore};

/// The writer for a resumed take: no second header, the shape store from the file, each kind's last time.
pub fn resumed_writer(existing: &[u8]) -> CursorWriter {
    let track = CursorTrack::read(existing, Limits::for_file_len(existing.len() as u64));
    CursorWriter::resumed(&track)
}

impl CursorWriter {
    /// `resumed_writer` from the file's track, as `run::resume` reads it (streamed, never the whole file in memory).
    pub fn resumed(track: &CursorTrack) -> Self {
        let last = track.last_times();
        let mut w = Self::new();
        w.header = track.header().is_some();
        w.shapes = ShapeStore::resumed(track);
        w.shapes_full = w.shapes.is_full();
        // What the file shows already, so a change back (a cursor hidden at the crash, shown again) is written.
        w.kept = Kept::shown_in(track);
        w.reorder = Reorder::resumed(&last);
        w.clicks = track.clicks_at(klipolo_model::time::Us::MAX);
        let area = track.area_at(klipolo_model::time::Us::MAX);
        w.area = (area.0 > 0.0 && area.1 > 0.0).then_some(area);
        if let Some((t, (x, y))) = last.moves.and_then(|t| Some((t, track.position(t)?))) {
            // The thinning goes on from the file's last position, and so does the still rule.
            let _ = w.thin.next((t, x, y));
            w.straight = Straight::after((t, x, y));
            (w.last_fed, w.last_move, w.place) = (Some(t), Some((t, x, y)), (x, y));
        }
        w
    }
}

#[cfg(test)]
mod tests {
    use klipolo_cursor::format::{Capture, Line, write_line};
    use klipolo_cursor::{ClickSource, CursorTrack, Limits};

    use super::resumed_writer;
    use crate::cursor::{CursorEvent, ShapeImage};
    use crate::preflight::cursor::CursorPlan;
    use crate::run::core::Effect;

    fn file(lines: &[Line]) -> Vec<u8> {
        let mut out = Vec::new();
        for line in lines {
            write_line(line, &mut out);
        }
        out
    }

    fn appended(fx: &[Effect]) -> Vec<u8> {
        let mut out = Vec::new();
        for e in fx {
            if let Effect::Append { bytes, .. } = e {
                out.extend_from_slice(bytes);
            }
        }
        out
    }

    fn header() -> Line {
        Line::Header {
            v: 2,
            capture: Capture::Separate,
            clicks: ClickSource::Events,
            w: 1440.0,
            h: 900.0,
        }
    }

    #[test]
    fn a_resumed_file_goes_on_without_a_second_header_or_older_lines() {
        let before = file(&[
            header(),
            Line::Move {
                t: 1_000_000,
                x: 0.2,
                y: 0.2,
            },
        ]);
        let mut w = resumed_writer(&before);
        let mut fx = Vec::new();
        w.open(
            CursorPlan::Separate,
            ClickSource::Events,
            (0.0, 0.0),
            &mut fx,
        );
        let mv = |x: f32| CursorEvent::Move {
            host_ns: 0,
            x,
            y: x,
        };
        // Older than the file's last move: stale, dropped. Then a move after a 2 s still.
        w.event(&mv(0.3), Some(900_000), &mut fx);
        w.event(&mv(0.4), Some(3_000_000), &mut fx);
        w.flush(&mut fx);
        let mut all = before.clone();
        all.extend(appended(&fx));
        let track = CursorTrack::read(&all[..], Limits::for_file_len(all.len() as u64));
        assert_eq!(track.notes().headers, 1);
        assert!(!track.notes().lost_any(), "{:?}", track.notes());
        let times: Vec<i64> = track.samples(i64::MIN, i64::MAX).map(|s| s.0).collect();
        assert_eq!(
            times,
            [1_000_000, 2_996_000, 3_000_000],
            "the held place before the move"
        );
    }

    #[test]
    fn a_resumed_file_without_a_header_gets_one_and_shape_ids_continue() {
        let def = |id: u16| Line::ShapeDef {
            id,
            key: format!("{:016x}", 100 + u64::from(id)),
            file: format!("cursors/{id}.png"),
            pw: 4,
            ph: 4,
            hx: 0.0,
            hy: 0.0,
            pts: 4.0,
            sys: None,
        };
        let before = file(&[def(0), def(1)]);
        let mut w = resumed_writer(&before);
        let mut fx = Vec::new();
        w.open(
            CursorPlan::Separate,
            ClickSource::Events,
            (1440.0, 900.0),
            &mut fx,
        );
        let image = ShapeImage {
            px: (4, 4),
            rgba: vec![255; 64],
            hotspot_px: (0.0, 0.0),
            pts: 4.0,
            sys: None,
        };
        let new = CursorEvent::NewShape {
            host_ns: 0,
            key: 7,
            image,
        };
        let known = CursorEvent::Shape {
            host_ns: 0,
            key: 101,
        };
        w.event(&new, Some(10), &mut fx);
        w.event(&known, Some(20), &mut fx);
        w.flush(&mut fx);
        assert!(
            fx.iter()
                .any(|e| matches!(e, Effect::WriteAtomic { key, .. } if key == "cursors/2.png"))
        );
        let mut all = before.clone();
        all.extend(appended(&fx));
        let track = CursorTrack::read(&all[..], Limits::for_file_len(all.len() as u64));
        let head = track.header().expect("a header for a file that had none");
        assert_eq!(
            (head.capture, head.area_pts),
            (Capture::Separate, (1440.0, 900.0))
        );
        assert_eq!(track.shape_at(10).map(|s| s.id), Some(2));
        assert_eq!(
            track.shape_at(20).map(|s| s.id),
            Some(1),
            "a key the file knew"
        );
        assert_eq!(
            track.shapes().iter().map(|s| s.id).collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }
}
