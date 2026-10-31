//! Copies whose record says their frames are numbered from the video stream's start, not the picture's (as copies were
//! made before every decoder numbered them from the picture's start).

use super::*;

/// An older copy (its record doesn't say it numbers frames from the picture's start) of a source
/// whose picture starts 70 ms in shows its frames 2 late: it is deleted at listing and made again in the background.
/// One of a source whose picture starts at 0 showed the same frames either way, and is kept and used as it is.
#[test]
fn an_older_copy_of_a_late_picture_is_made_again_and_one_of_a_picture_at_0_is_kept() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (late, at_0) = (dir.path().join("late.mp4"), dir.path().join("at-0.mp4"));
        overwrite(&late, b"a phone clip whose picture starts after its sound");
        overwrite(&at_0, b"a clip whose picture starts at 0");
        let root = dir.path().join("media/proxies");
        let older = |source: &Path| {
            let copy = finished_copy(&root, source);
            let mut record = CopyRecord::read(&copy).unwrap();
            record.from_stream_start = false;
            record.write(&copy).unwrap();
            copy
        };
        let (late_copy, at_0_copy) = (older(&late), older(&at_0));
        let late_source = late.clone();
        let probe = move |path: &Path| {
            let mut probed = facts(path)?;
            if path == late_source {
                probed.start_us = 70_000;
            }
            Ok(probed)
        };

        let worker = Arc::new(StandIn::default());
        let jobs = scheduler_probing(&root, &worker, probe);
        let (_publish, rx) = watch::channel(feed(&[&late, &at_0]));
        jobs.watch(rx);
        eventually("both files are listed", &jobs, || {
            jobs.list().jobs.len() == 2
        })
        .await;
        assert!(
            !late_copy.exists(),
            "the late picture's older copy is deleted at listing"
        );
        assert_eq!(
            state_of(&jobs.list().jobs, &late),
            Some(MediaJobState::Waiting)
        );
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        eventually("the late picture's copy is made again", &jobs, || {
            jobs.list()
                .jobs
                .iter()
                .all(|j| j.state == MediaJobState::Done)
        })
        .await;
        let launched: Vec<String> = worker
            .launched
            .lock()
            .unwrap()
            .iter()
            .map(|(stem, ..)| stem.clone())
            .collect();
        assert_eq!(launched.len(), 1, "only the late picture's copy is made");
        assert_eq!(
            Some(launched[0].as_str()),
            late_copy.file_stem().and_then(|s| s.to_str())
        );
        assert!(CopyRecord::read(&late_copy).unwrap().from_stream_start);
        assert_eq!(std::fs::read(&at_0_copy).unwrap(), b"a copy made earlier");
        eventually("both copies are in use", &jobs, || {
            let map = preview.last();
            map.contains_key(&*late.to_string_lossy()) && map.contains_key(&*at_0.to_string_lossy())
        })
        .await;
    });
}
