//! What numbering frames from the stream's start means for copies and jobs made before it: decoders then numbered a source's frames
//! from the file's 0, so where the picture starts later, an older copy's frames (and an older job's finished segments)
//! are that many frames late against the source, which every decoder now numbers from its picture's start.

use std::path::Path;

use klipolo_model::time::Us;

use super::facts::{SourceFacts, probe_facts};
use super::job::ProxyJob;
use crate::export::shared::FrameRate;

/// A picture starting `start_us` into its file was numbered differently by the old numbering at `rate`: its start
/// rounds to at least one frame (the old numbering shifted every frame by exactly that rounding, so a start under half
/// a frame numbered the same frames either way).
pub(super) fn starts_late(start_us: Us, rate: FrameRate) -> bool {
    (start_us as f64 * rate.f64() / 1e6).round() != 0.0
}

/// Whether an unfinished job starts again instead of resuming: it was made with the old numbering and its
/// source's picture `starts_late`, so its finished segments would be mixed into the copy late. `facts`: the source's,
/// as it is now (probed here when not given; a source that can't be read keeps its job).
pub(super) fn restarts(job: &ProxyJob, facts: Option<&SourceFacts>) -> bool {
    if job.from_stream_start {
        return false;
    }
    let probed;
    let facts = match facts {
        Some(facts) => facts,
        None => match probe_facts(Path::new(&job.source)) {
            Ok(facts) => {
                probed = facts;
                &probed
            }
            Err(_) => return false,
        },
    };
    starts_late(facts.start_us, job.rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::job::PartStatus;
    use crate::media::key::source_id;
    use crate::media::proxy::decide::ProxyPolicy;
    use crate::media::proxy::job::{CopyRecord, Opened};
    use crate::platform::gpu::GpuClass;

    const NTSC: FrameRate = FrameRate {
        num: 30_000,
        den: 1001,
    };

    /// 20 s of 4K at 29.97 fps (Intel needs a copy) whose picture starts `start_us` in, of the file at `path`.
    fn facts(path: &Path, start_us: Us) -> SourceFacts {
        SourceFacts {
            width: 3840,
            height: 2160,
            rate: NTSC,
            codec: "h264".into(),
            ten_bit: false,
            ten_bit_hw: false,
            hdr: false,
            max_key_interval_us: 500_000,
            variable_rate: false,
            hw_decodes: true,
            duration_us: 20_000_000,
            start_us,
            id: path.to_str().and_then(source_id),
        }
    }

    fn policy() -> ProxyPolicy {
        ProxyPolicy::with_encoder(GpuClass::Integrated, true)
    }

    /// At 29.97 fps half a frame is 16.68 ms. An older copy of a picture starting 16 ms in showed the
    /// same frames and stays current; one starting 17 ms in is made again. A copy made since is current whatever the
    /// start, and without the facts (the preview's own check) nothing changes.
    #[test]
    fn an_older_copy_is_made_again_from_half_a_frame_of_start() {
        assert!(!starts_late(16_000, NTSC) && starts_late(17_000, NTSC));
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("clip.mp4");
        std::fs::write(&source, b"a source").unwrap();
        let root = dir.path().join("proxies");
        let job = ProxyJob::create(
            &root,
            source.to_str().unwrap(),
            &facts(&source, 0),
            &policy(),
        )
        .unwrap();
        let record = |from_stream_start| CopyRecord {
            from_stream_start,
            ..CopyRecord::of(&job)
        };
        let current = |record: &CopyRecord, start_us| {
            let facts = facts(&source, start_us);
            record.current(
                job.source.as_str(),
                &job.out,
                Some(job.source_id),
                Some(&facts),
            )
        };
        assert!(current(&record(false), 16_000));
        assert!(!current(&record(false), 17_000));
        assert!(current(&record(true), 70_000));
        assert!(record(false).current(&job.source, &job.out, Some(job.source_id), None));
    }

    /// An unfinished job made with the old numbering (its manifest doesn't say how it numbers frames) of a
    /// source whose picture starts 70 ms in starts again with every segment to do, and is marked; one of a source whose
    /// picture starts at 0 resumes with its finished segment.
    #[test]
    fn an_older_job_of_a_late_picture_starts_again_and_one_of_a_picture_at_0_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proxies");
        for (name, start_us, again) in [("late.mp4", 70_000, true), ("at-0.mp4", 0, false)] {
            let source = dir.path().join(name);
            std::fs::write(&source, b"a source").unwrap();
            let facts = facts(&source, start_us);
            let path = source.to_str().unwrap();
            let mut older = ProxyJob::create(&root, path, &facts, &policy()).unwrap();
            older.from_stream_start = false;
            older.segment_done(0, (1, 0), None);
            older.save().unwrap();
            let job = match ProxyJob::open_with(&root, path, Some(&facts), &policy()) {
                Opened::Fresh(job) if again => job,
                Opened::Resumed(job) if !again => job,
                other => panic!("{name}: {other:?}"),
            };
            assert_eq!(job.from_stream_start, again, "{name}");
            let first = job.segments[0].status;
            assert_eq!(first == PartStatus::Done, !again, "{name}: {first:?}");
            assert_eq!(
                ProxyJob::open(&job.dir).unwrap().from_stream_start,
                again,
                "{name}"
            );
        }
    }
}
