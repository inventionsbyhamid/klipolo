//! A pass's pictures, chunk by chunk: the decoder (opened again at a chunk's start when it must be), the differ it
//! feeds, and the minute it can't read (`Unreadable`, passed over by `Pass::write`).

use klipolo_cursor::activity::{CHUNK_TICKS, Differ, TICK_US, Tick};
use klipolo_model::time::Us;

use super::{Ended, Pace, Pass};
use crate::media::activity::files::CHUNK_US;
use crate::platform::activity::{ActivityFrames, LUMA_BYTES};

/// What became of a chunk (`Pass::ticks`).
pub(super) enum Chunk {
    Made,
    /// Its pictures couldn't be read (damaged data): it is passed over.
    Lost(String),
}

/// A pass's decoder and the differ it feeds.
pub(super) struct Decoding {
    /// None after a minute it couldn't read: the next chunk opens one at its own start.
    pub(super) frames: Option<Box<dyn ActivityFrames>>,
    /// The next picture's time.
    pub(super) at: Us,
    pub(super) differ: Differ,
    pub(super) luma: Box<[u8; LUMA_BYTES]>,
    /// The chunk whose first picture the differ holds (the chunk after the one just made).
    pub(super) primed: Option<u32>,
    /// The video gave its last picture: the chunks after are made empty, with no decoder opened for them.
    pub(super) ended: bool,
}

impl Pass<'_> {
    /// Chunk `chunk`'s ticks into `out` (fewer only where the video ends), from `d`'s decoder, opened again at the
    /// chunk's start unless the differ holds its first picture. A picture it can't read loses the minute
    /// (`Chunk::Lost`); a decoder that opened before and won't now (a drive gone) ends the pass (`Unreadable`).
    pub(super) fn ticks(
        &self,
        d: &mut Decoding,
        chunk: u32,
        duration: Us,
        out: &mut Vec<Tick>,
    ) -> Result<Chunk, Ended> {
        let start = chunk as Us * CHUNK_US;
        out.clear();
        if !d.ended && d.primed != Some(chunk) {
            if d.at != start || d.frames.is_none() {
                // Release the decoder before the next one opens: one at a time.
                d.frames = None;
                d.frames = Some(self.open(start).map_err(|e| match e {
                    Ended::Failed(e) => Ended::Unreadable(e),
                    e => e,
                })?);
            }
            d.differ.reset();
            d.at = start;
            match self.picture(d) {
                Ok(true) => {
                    d.differ.push(&d.luma);
                }
                // No picture at 0 is no picture at all (a tick before the first frame shows it).
                Ok(false) if start == 0 => {
                    return Err(Ended::Failed("no picture can be decoded".into()));
                }
                Ok(false) => d.ended = true,
                Err(Ended::Unreadable(e)) => return Ok(Chunk::Lost(e)),
                Err(e) => return Err(e),
            }
        }
        while !d.ended && out.len() < CHUNK_TICKS as usize && d.at < duration {
            match self.picture(d) {
                Ok(true) => out.push(d.differ.push(&d.luma).unwrap_or(Tick::UNKNOWN)),
                Ok(false) => d.ended = true,
                Err(Ended::Unreadable(e)) => return Ok(Chunk::Lost(e)),
                Err(e) => return Err(e),
            }
        }
        // A whole minute leaves the next chunk's first picture in the differ.
        d.primed = (out.len() == CHUNK_TICKS as usize).then_some(chunk + 1);
        Ok(Chunk::Made)
    }

    /// The next picture (true), or the end of the video (false); waits or stops as `pace` says first.
    fn picture(&self, d: &mut Decoding) -> Result<bool, Ended> {
        match (self.pace)() {
            Pace::Go => {}
            Pace::Held => return Err(Ended::Held),
            Pace::Stop => return Err(Ended::Stopped),
        }
        let Some(frames) = d.frames.as_mut() else {
            return Ok(false);
        };
        match frames.next(&mut d.luma)? {
            Some(t) => {
                debug_assert_eq!(t, d.at, "a picture a tick");
                d.at += TICK_US;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}
