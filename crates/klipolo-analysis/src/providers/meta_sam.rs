//! SAM 3.1 as the `SubjectTracker`, through the Meta adapter's client and slots: the video is uploaded (Files API, or
//! through the relay its `sam/videos`, which writes its own copy so it knows the frames Meta will bill),
//! SAM is asked for every match of the phrase on every frame (read from its streamed events to the final usage, as the
//! relay forwards SAM video calls only streamed; masks one bit a pixel and skipped), and the upload is deleted on every
//! path, as the scene log's are. A video call holds the device's video slot from its
//! upload to its answer. SAM's boxes come back in the video's pixels; they leave here as parts of the picture.

use std::path::Path;

use klipolo_meta::{MetaError, ResponseOut, pricing};

use super::meta::{Meta, Upload, provider_error};
use super::{
    BoxFuture, ProviderError, SubjectTracker, TrackedBox, TrackedFrame, Tracking, retry_busy,
};

/// The most frames SAM takes in one video.
pub const SAM_MAX_FRAMES: u64 = 15_000;
/// Through Klipolo's relay, which makes at most ten minutes of video for SAM: 1,180 frames at 2 a second, under its
/// ten-minute check with a frame to spare. Longer footage goes in parts, and their tracks join.
pub const RELAY_MAX_FRAMES: u64 = 1_180;

impl SubjectTracker for Meta {
    fn model(&self) -> &str {
        klipolo_sam::MODEL
    }

    fn max_frames(&self) -> u64 {
        if self.client().is_relay() {
            RELAY_MAX_FRAMES
        } else {
            SAM_MAX_FRAMES
        }
    }

    /// SAM's list price: $0.20 a thousand frames, at least 50 billed a video.
    fn estimate_usd(&self, frames: u64) -> f64 {
        pricing::sam_frames_usd(frames)
    }

    fn track<'a>(
        &'a self,
        video: &'a Path,
        phrase: &'a str,
    ) -> BoxFuture<'a, Result<Tracking, ProviderError>> {
        Box::pin(async move {
            let _video = self.videos.acquire().await;
            let id = retry_busy(|| self.call(upload_video(self.client(), video))).await?;
            let upload = Upload {
                meta: self.clone(),
                id: Some(id),
            };
            let body = klipolo_sam::concept_request(upload.id(), phrase);
            let answer = retry_busy(|| self.call(self.client().respond_stream(body.clone()))).await;
            upload.delete().await;
            tracking(answer?, self.client().is_relay())
        })
    }
}

/// Uploads the small copy for SAM (through the relay, the relay makes its own copy of it). A file that can't be read
/// is named as what it is, not by its temporary name.
async fn upload_video(
    client: &klipolo_meta::MetaClient,
    video: &Path,
) -> Result<String, MetaError> {
    client.upload_sam_video(video).await.map_err(|e| match e {
        MetaError::LocalFile { .. } => MetaError::LocalFile {
            name: "this clip's small copy".into(),
        },
        e => e,
    })
}

/// SAM's frame lines as boxes in parts of the picture, priced by the frames it billed; `relay` when it came through
/// Klipolo's relay. The answer is paid for, so a line that doesn't read (one cut off part-way, say) is skipped and the
/// rest kept; only an answer of which no line reads fails. One Meta stopped part-way (`incomplete`) is said to be cut
/// off, so only what it covers is kept as tracked.
fn tracking(out: ResponseOut, relay: bool) -> Result<Tracking, ProviderError> {
    let text = out.output_text.as_deref().unwrap_or_default();
    let (frames, skipped) = klipolo_sam::parse_frames_lenient(text);
    if let Some(first) = skipped.first() {
        if frames.is_empty() {
            return Err(provider_error(
                MetaError::Decode(format!("SAM: {first}")),
                relay,
            ));
        }
        tracing::warn!(skipped = skipped.len(), %first, "SAM: lines of a paid answer didn't read; kept the rest");
    }
    let frames = frames
        .into_iter()
        .map(|f| TrackedFrame {
            frame: f.frame,
            boxes: f
                .objects
                .into_iter()
                .map(|o| {
                    let (w, h) = (f64::from(o.frame_w), f64::from(o.frame_h));
                    TrackedBox {
                        id: o.id,
                        left: f64::from(o.bbox.x0) / w,
                        top: f64::from(o.bbox.y0) / h,
                        right: f64::from(o.bbox.x1) / w,
                        bottom: f64::from(o.bbox.y1) / h,
                        confidence: o.confidence,
                    }
                })
                .collect(),
        })
        .collect();
    Ok(Tracking {
        frames,
        frames_billed: out.usage.video_frames_processed,
        cost_usd: pricing::sam_usd(out.usage),
        cut_off: out.status.as_deref() == Some("incomplete"),
    })
}
