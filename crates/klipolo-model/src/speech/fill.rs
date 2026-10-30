//! Captions over a recording clip's true gaps: where the camera's own sound fills a gap in the recorded
//! sound (`SourceMix::fills_gaps`, `Recording::true_gaps`), it is what plays there, so its transcript is what the
//! captions read (and what a word cut cuts).

use super::map::VoiceRun;
use crate::ids::Id;
use crate::project::{Project, Recording, RecordingSource, SourceRun};
use crate::time::Us;
use crate::timeline::Clip;

impl VoiceRun {
    /// `run` of `source` as a voice run over its recording-time stretch inside `[from, to)`: its drift the rate, its
    /// nudge in its offset, and its file time less its file's sound lead (sound time). None outside it.
    pub(crate) fn of(
        p: &Project,
        source: &RecordingSource,
        run: &SourceRun,
        (from, to): (Us, Us),
    ) -> Option<Self> {
        let (start, end) = run.rec_span(source.nudge_us);
        let (rec_from, rec_to) = (start.max(from), end.min(to));
        let lead = p.asset(&run.asset_id).map_or(0, |a| a.lead_us());
        (rec_from < rec_to).then(|| Self {
            asset: run.asset_id.clone(),
            rec_from,
            rec_to,
            offset_us: start,
            rate: 1.0 + run.drift_ppm * 1e-6,
            file_in_us: run.file_in_us.saturating_sub(lead),
        })
    }
}

/// The voice runs of `clip` (a clip of `rec`) with a filling picture source's runs over the clip's true gaps, in
/// recording order. A picture source fills there when its mix in the clip is heard there: muted with `fills_gaps`, or
/// not muted (it plays everywhere then). Nothing changes when the voice already is that source (nothing else has
/// sound), or when no source fills. O(sources × runs × gaps), and nothing allocated for a clip without a fill.
pub(crate) fn with_fills(
    p: &Project,
    clip: &Clip,
    rec: &Recording,
    voice: Vec<VoiceRun>,
) -> Vec<VoiceRun> {
    let sounding = |id: &Id| p.asset(id).is_some_and(|a| a.has_audio);
    let filling = rec.sources.iter().find(|s| {
        let mix = clip.mix_for(s);
        s.role.is_video()
            && mix.volume > 0.0
            && (!mix.muted || mix.fills_gaps)
            && s.runs.iter().any(|r| sounding(&r.asset_id))
    });
    let Some(picture) = filling else {
        return voice;
    };
    if voice
        .iter()
        .any(|v| picture.runs.iter().any(|r| r.asset_id == v.asset))
    {
        return voice;
    }
    let gaps = rec.true_gaps(clip.source_in_us, clip.source_out_us);
    if gaps.is_empty() {
        return voice;
    }
    let mut runs = voice;
    for &gap in &gaps {
        let heard = picture.runs.iter().filter(|r| sounding(&r.asset_id));
        runs.extend(heard.filter_map(|run| VoiceRun::of(p, picture, run, gap)));
    }
    runs.sort_by_key(|r| r.rec_from);
    runs
}
