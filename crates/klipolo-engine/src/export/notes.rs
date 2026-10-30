//! What a finished export says it drew reduced: the worker records each note in
//! the job once, with the first frame's time, and saves it, so a resumed run keeps it and `deliver` sends it with
//! `ExportEvent::Finished`.

use klipolo_model::time::Us;

use super::shared::{JobHandle, WorkerError};
use crate::api::{CodedError, Said};

impl JobHandle {
    /// Records note `code` at `at_us` unless the job has it already (saved with the job).
    pub fn note(&self, code: &str, at_us: Us) -> Result<(), WorkerError> {
        let known = |n: &Said| matches!(n, Said::Coded(c) if c.code == code);
        if self.read(|j| j.notes.iter().any(known)) {
            return Ok(());
        }
        self.update(|j| {
            j.notes
                .push(CodedError::new(code).with("atUs", at_us).into())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use klipolo_model::project::{FormatKind, Project};

    use super::*;
    use crate::api::{EXPORT_SCENE_REDUCED, ExportQuality};
    use crate::export::job::ExportJob;
    use crate::export::naming::plan_files;
    use crate::export::runner::Batch;

    /// A note is kept once, saved with the job, and still there when the job is opened again (a resumed run).
    #[test]
    fn a_note_is_kept_once_and_saved_with_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::new("Notes");
        let files = plan_files(
            &p,
            &[FormatKind::Landscape],
            ExportQuality::High,
            dir.path(),
        );
        let batch = Batch {
            project: p,
            fps: 30,
            quality: ExportQuality::High,
            files,
            pcm_dir: None,
            speech: Arc::default(),
            text_language: "en".into(),
            contribs: Default::default(),
            caption_file: None,
            allow_stand_ins: false,
            activity_dir: None,
            allow_missing_cursor: false,
        };
        let job = ExportJob::create(&batch, 10).unwrap();
        let dir = job.dir.clone();
        let handle = JobHandle::new(job);
        handle.note(EXPORT_SCENE_REDUCED, 2_000_000).unwrap();
        handle.note(EXPORT_SCENE_REDUCED, 3_000_000).unwrap();
        let reopened = ExportJob::open(&dir).unwrap();
        assert_eq!(
            reopened.notes,
            [Said::Coded(
                CodedError::new(EXPORT_SCENE_REDUCED).with("atUs", 2_000_000i64)
            )]
        );
    }
}
