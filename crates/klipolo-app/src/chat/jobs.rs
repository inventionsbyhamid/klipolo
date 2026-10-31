//! A message's jobs (`klipolo_chat::Job`): work for a runner rather than ops. Following a subject and blurring aren't
//! in Klipolo yet, so the reply says they are coming and nothing changes for them. A picture to draw goes to
//! `pictures` (`PictureAsk`), which draws it when the page asks and places it on Use.
//!
//! Every kind of job is matched in `one`, for chat's messages (`start`) and other apps' calls (`picture_for`): a new kind
//! of job the compiler makes doesn't build until it is handled here.

use klipolo_chat::{Compiled, Job};

use super::media_jobs;
use super::pictures::PictureAsk;

/// Said for `followSubject`.
pub const FOLLOW_COMING: &str = "Following a subject isn't in Klipolo yet. It's coming soon.";
/// Said for `blurObjects`.
pub const BLUR_COMING: &str = "Blurring isn't in Klipolo yet. It's coming soon.";

/// What a message's jobs came to.
#[derive(Default)]
pub(crate) struct Started {
    /// Pictures to draw.
    pub pictures: Vec<PictureAsk>,
    /// A sentence for each kind of job Klipolo can't run yet.
    pub coming: Vec<&'static str>,
    /// Their tools, for the turn's note.
    tools: Vec<&'static str>,
}

impl Started {
    /// The turn's note on the jobs not run.
    pub fn note(&self) -> String {
        format!(
            "Not available yet, nothing done: {}.",
            self.tools.join(", ")
        )
    }
}

/// Starts `compiled`'s jobs.
pub(crate) fn start(compiled: &Compiled) -> Started {
    let mut started = Started::default();
    for job in &compiled.jobs {
        match one(job) {
            Ok(ask) => started.pictures.push(ask),
            Err((said, tool)) => {
                if !started.coming.contains(&said) {
                    started.coming.push(said);
                    started.tools.push(tool);
                }
            }
        }
    }
    started
}

/// One job as chat runs it: the picture to draw, or the sentence for a kind Klipolo can't run yet. Another app's job
/// (MCP) is run the same way.
pub fn picture_for(job: &Job) -> Result<PictureAsk, &'static str> {
    one(job).map_err(|(said, _)| said)
}

/// `job`'s picture, or what is said for it with its tool's name.
fn one(job: &Job) -> Result<PictureAsk, (&'static str, &'static str)> {
    match job {
        Job::Follow { .. } => Err((FOLLOW_COMING, "followSubject")),
        Job::Blur { .. } => Err((BLUR_COMING, "blurObjects")),
        Job::MakeSticker(sticker) => Ok(media_jobs::sticker(sticker)),
        Job::MakeTitleBackground {
            prompt, title_id, ..
        } => Ok(media_jobs::title_background(prompt, title_id)),
    }
}
