//! Opening a pass's encoders: one session per format, all at once.

use std::sync::Arc;

use super::Pass;
use crate::export::encode::{EncoderSetup, EncoderThread};
use crate::export::protocol::ErrorCode;
use crate::export::quality::EncodeSettings;
use crate::export::segment::{SegmentLedger, SegmentPlan};
use crate::export::shared::{FrameRate, JobHandle, internal};

/// Opens the pass's encoders (`software[i]`: output i uses VideoToolbox's software encoder). Finished segments from
/// another encoder stay finished until this one has produced verified output (`JobHandle::segment_done`).
pub(super) async fn open_encoders(
    job: &JobHandle,
    pass: &[usize],
    settings: &[EncodeSettings],
    software: &[bool],
) -> Result<Vec<EncoderThread>, Pass> {
    let (mut encoders, mut opened) = (Vec::new(), Vec::new());
    for &i in pass {
        let (plans, setup) = job.read(|j| {
            let o = &j.outputs[i];
            let plans: Vec<SegmentPlan> = o
                .pending()
                .map(|s| SegmentPlan {
                    index: s.index,
                    start_frame: s.start_frame,
                    frames: s.frames,
                    path: j.path(&s.file),
                })
                .collect();
            let setup = EncoderSetup {
                output: i,
                settings: settings[i],
                software: software[i],
                size: (o.width, o.height),
                rate: FrameRate::whole(j.fps),
                segment_frames: j.segment_frames,
                options: &[],
            };
            (plans, setup)
        });
        let ledger: Arc<dyn SegmentLedger> = Arc::new(job.clone());
        let (encoder, open) = EncoderThread::start(ledger, setup, plans)?;
        encoders.push(encoder);
        opened.push(open);
    }
    for open in opened {
        match open.await {
            Ok(Ok(_config)) => {}
            Ok(Err(e)) if e.code == ErrorCode::EncoderUnavailable && pass.len() > 1 => {
                return Err(Pass::Sequential);
            }
            Ok(Err(e)) => return Err(Pass::Failed(e)),
            Err(_) => return Err(Pass::Failed(internal("the encoder stopped while opening"))),
        }
    }
    Ok(encoders)
}
