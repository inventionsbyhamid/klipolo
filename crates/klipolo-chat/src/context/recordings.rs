//! What the context says of a recording's clip, in the recording tools' words: the recording's name ("Recording 2", by
//! its place among the project's recordings, as Klipolo names it everywhere), how its camera shows and what its sounds
//! play there.
//! - The camera: how the main format shows it (`camera bubble bottomRight circle`: for a bubble, its place and shape),
//!   then each other format of the video that shows it another way (`landscape splitBottom`), its border (`border on`)
//!   while a format shows the bubble, and its mirror (`mirrored`, `not mirrored`) while one shows the camera; `camera
//!   on its own layer` once it is separated, and `camera only, mirrored` when it is the clip's footage.
//! - The sounds the tools name, in the recording's order, a second one of a kind numbered (`mic 2`, as `which` takes
//!   it): each `at 50%` when it isn't as recorded and `muted` when it is, both as the clip plays it (its own mix, else
//!   the source's), or `on its own track` once separated there.
//!
//! A cut take is many clips of one recording that mostly read the same: a clip whose camera and sounds read as an
//! earlier clip's of the same recording says `as c1` (`Told`), so a take in 80 pieces costs a few words a piece.
//!
//! `camera on its own layer` and `on its own track` read the compiler's own tests (`every_camera`, `OwnTrack`), so the
//! context says what the tools will find. Built once per context: O(clips + layers), plus a binary search per clip and
//! source.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use klipolo_model::Project;
use klipolo_model::ids::Id;
use klipolo_model::time::Us;
use klipolo_model::timeline::{CameraMode, Clip, ClipSource};

use crate::compile::{OwnTrack, every_camera, kind_of};
use crate::tools::Sound;
use crate::tools::recorded::{anchor_word, mode_word, shape_word};

/// How a clip's camera shows, as the compiler finds it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Camera {
    OnClip,
    Separate,
    Footage,
}

/// What the clip lines written so far said of each recording: the first clip's ref for each reading, by recording id.
pub(super) type Told<'p> = HashMap<(&'p str, String), String>;

/// The recordings of one project, read once for every clip line.
pub(super) struct Recordings<'p> {
    p: &'p Project,
    cameras: HashMap<Id, Camera>,
    /// Each recording's sounds the tools name, in order: the source, its name in the context, and where it plays from
    /// its own track.
    sounds: HashMap<
        &'p str,
        Vec<(
            &'p klipolo_model::project::RecordingSource,
            String,
            OwnTrack,
        )>,
    >,
}

impl<'p> Recordings<'p> {
    pub(super) fn of(p: &'p Project) -> Self {
        let cams = every_camera(p, None);
        let mut cameras = HashMap::new();
        cameras.extend(cams.on_clips.into_iter().map(|id| (id, Camera::OnClip)));
        cameras.extend(
            cams.separate
                .into_iter()
                .map(|(id, _)| (id, Camera::Separate)),
        );
        cameras.extend(cams.footage.into_iter().map(|id| (id, Camera::Footage)));
        let mut sounds = HashMap::new();
        for r in &p.recordings {
            let mut seen: HashMap<Sound, usize> = HashMap::new();
            let named = (r.sources.iter()).filter_map(|s| {
                let kind = kind_of(p, s)?;
                let n = seen.entry(kind).or_default();
                *n += 1;
                let name = match *n {
                    1 => kind.name().to_owned(),
                    n => format!("{} {n}", kind.name()),
                };
                Some((s, name, OwnTrack::of(p, s)))
            });
            sounds.insert(r.id.as_str(), named.collect());
        }
        Self { p, cameras, sounds }
    }

    /// What the line of clip `clip` (ref `short`, starting at `start`) says of its recording: its camera and sounds
    /// (`notes`), or `as c1` when an earlier line of the same recording (in `said`) read the same.
    pub(super) fn told(
        &self,
        clip: &'p Clip,
        (short, start): (&str, Us),
        told: &mut Told<'p>,
    ) -> Vec<String> {
        let ClipSource::Recording { id } = &clip.source else {
            return Vec::new();
        };
        let notes = self.notes(clip, start);
        match told.entry((id.as_str(), notes.join("; "))) {
            Entry::Occupied(first) => vec![format!("as {}", first.get())],
            Entry::Vacant(first) => {
                first.insert(short.to_owned());
                notes
            }
        }
    }

    /// What clip `clip` (starting at `start`) shows of its recording's camera and plays of its sounds, each part on its
    /// own (`camera ...`, `sounds ...`); nothing for a clip that isn't a recording's.
    fn notes(&self, clip: &Clip, start: Us) -> Vec<String> {
        let ClipSource::Recording { id } = &clip.source else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(camera) = self.camera(clip) {
            out.push(camera);
        }
        let end = start.saturating_add(clip.duration_us());
        let sounds: Vec<String> = (self.sounds.get(id.as_str()).into_iter().flatten())
            .map(|(source, name, own)| {
                if own.over(start, end) {
                    return format!("{name} on its own track");
                }
                let mix = clip.mix_for(source);
                let mut said = name.clone();
                if mix.volume != 1.0 {
                    said.push_str(&format!(" at {}%", (mix.volume * 100.0).round()));
                }
                if mix.muted {
                    said.push_str(" muted");
                }
                said
            })
            .collect();
        if !sounds.is_empty() {
            out.push(format!("sounds {}", sounds.join(", ")));
        }
        out
    }

    /// `camera bubble bottomRight circle, portrait splitBottom, border on, mirrored`, `camera on its own layer` or
    /// `camera only, mirrored`.
    fn camera(&self, clip: &Clip) -> Option<String> {
        let layout = clip.camera.as_ref()?;
        let mirror = if layout.mirror {
            "mirrored"
        } else {
            "not mirrored"
        };
        match self.cameras.get(&clip.id)? {
            Camera::Separate => return Some("camera on its own layer".to_owned()),
            Camera::Footage => return Some(format!("camera only, {mirror}")),
            Camera::OnClip => {}
        }
        let shown = |mode: CameraMode| match mode {
            CameraMode::Bubble => format!(
                "bubble {} {}",
                anchor_word(layout.bubble.anchor),
                shape_word(layout.shape)
            ),
            other => mode_word(other).to_owned(),
        };
        let formats = &self.p.settings.formats;
        let shows = |mode: CameraMode| formats.iter().any(|f| layout.mode_for(*f) == mode);
        let main = layout.mode_for(self.p.settings.primary_format);
        let mut parts = vec![shown(main)];
        for f in formats {
            let mode = layout.mode_for(*f);
            if mode != main {
                parts.push(format!("{} {}", format_word(*f), shown(mode)));
            }
        }
        if shows(CameraMode::Bubble) {
            parts.push(
                if layout.border {
                    "border on"
                } else {
                    "border off"
                }
                .to_owned(),
            );
        }
        if shows(CameraMode::Bubble) || shows(CameraMode::SplitBottom) {
            parts.push(mirror.to_owned());
        }
        Some(format!("camera {}", parts.join(", ")))
    }
}

/// "Recording 2" for `p`'s second recording.
pub(super) fn recording_name(p: &Project, id: &str) -> String {
    (p.recordings.iter().position(|r| r.id == id)).map_or_else(
        || "missing recording".to_owned(),
        |n| format!("Recording {}", n + 1),
    )
}

fn format_word(f: klipolo_model::project::FormatKind) -> &'static str {
    use klipolo_model::project::FormatKind;
    match f {
        FormatKind::Landscape => "landscape",
        FormatKind::Portrait => "portrait",
        FormatKind::Square => "square",
    }
}
