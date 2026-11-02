//! A video pipe's encoder at start: the take's `VideoSpec::encoders` are tried in order, so an
//! encoder that won't start (its id gone, or HEVC refused after the probe cached it, as on an Intel Mac with a T2)
//! never leaves the take without video while another one works. Only an encoder's own refusal moves on to the next; a
//! permission, a missing display or the OS's error ends the start as it is. Recording on any but the first, the pipe
//! sends the mild `capture.note.encoderFallback` once.

use std::sync::mpsc::SyncSender;

use crate::backend::EncoderPick;
use crate::pipe::{PipeError, PipeEvent};

/// The note a pipe sends when it records on another encoder than the probe's choice.
pub const FALLBACK_NOTE: &str = "capture.note.encoderFallback";

/// `start` with the first of `picks` it accepts, and that pick's place in the list. With none listed, `start` gets
/// None: the backend's own pick. A refusal (`PipeError::Encoder`) tries the next; any other error, or the last
/// refusal, is returned. Recording on any but the first sends `FALLBACK_NOTE` to `events`.
pub fn start_first<T>(
    picks: &[EncoderPick],
    events: &SyncSender<PipeEvent>,
    mut start: impl FnMut(Option<&EncoderPick>) -> Result<T, PipeError>,
) -> Result<(T, usize), PipeError> {
    if picks.is_empty() {
        return start(None).map(|started| (started, 0));
    }
    let mut refused = None;
    for (at, pick) in picks.iter().enumerate() {
        match start(Some(pick)) {
            Ok(started) => {
                if at > 0 {
                    tracing::warn!(id = pick.id, refused = ?refused, "encoder: the take records on a fallback");
                    let _ = events.try_send(PipeEvent::Note(FALLBACK_NOTE.into()));
                }
                return Ok((started, at));
            }
            Err(e @ PipeError::Encoder(_)) => {
                tracing::warn!(id = pick.id, error = %e, "encoder: refused at the take's start");
                refused = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(refused.unwrap_or(PipeError::Unsupported))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use super::*;
    use crate::media::Codec;

    fn pick(id: &str) -> EncoderPick {
        EncoderPick {
            codec: if id.starts_with("hevc") {
                Codec::Hevc
            } else {
                Codec::H264
            },
            id: id.into(),
        }
    }

    #[test]
    fn a_refused_encoder_gives_way_to_the_next_and_says_so_once() {
        let (events, notes) = sync_channel(4);
        let picks = [pick("hevc.ave"), pick("hevc.gva"), pick("h264.gva")];
        let mut tried = vec![];
        let got = start_first(&picks, &events, |p| {
            let id = p.unwrap().id.clone();
            tried.push(id.clone());
            if id.starts_with("hevc") {
                Err(PipeError::Encoder(-12_900))
            } else {
                Ok(id)
            }
        });
        assert_eq!(got, Ok(("h264.gva".to_string(), 2)));
        assert_eq!(tried, ["hevc.ave", "hevc.gva", "h264.gva"]);
        assert_eq!(
            notes.try_iter().collect::<Vec<_>>(),
            [PipeEvent::Note(FALLBACK_NOTE.into())]
        );

        let first = start_first(&picks, &events, |_| Ok(()));
        assert_eq!(first, Ok(((), 0)));
        assert!(
            notes.try_iter().next().is_none(),
            "the probe's choice: no note"
        );
        let denied = start_first(&picks, &events, |_| {
            Err::<(), _>(PipeError::PermissionDenied)
        });
        assert_eq!(
            denied,
            Err(PipeError::PermissionDenied),
            "not the encoder's refusal: no other is tried"
        );
        let none = start_first(&[], &events, |p| Ok(p.is_none()));
        assert_eq!(
            none,
            Ok((true, 0)),
            "nothing listed: the backend's own pick"
        );
    }
}
