//! What every frame of one project revision shares: the project, its zooms (springs and camera runs), clip starts,
//! transition windows, strings and caption pages, the cursor takes it plans with and their follow paths, and (the
//! preview's only) the editing copies its sources are read from.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_model::captions::Captions;
use klipolo_model::project::Project;
use klipolo_model::time::Us;
use klipolo_model::timeline::Solo;
use klipolo_model::transitions::TransitionWindow;

use super::captions::CaptionLook;
use super::strings::Strings;
use super::takes::{Follows, Takes};
use super::zoom;
use crate::media::cursor::CursorTracks;
use crate::registry::{Registry, ShaderEffect, ShaderKind};

/// An editing copy the preview reads in place of its source. Copies are stored upright (the worker bakes the
/// turn in) and numbered on their own grid (a 29.97 source's copy runs at 30), so a source read from one has no turn and
/// the copy's own rate.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaCopy {
    pub path: Arc<str>,
    /// The copy's frame rate (0 = unknown: the decoder pool probes it).
    pub fps: f64,
}

impl MediaCopy {
    pub fn new(path: impl Into<Arc<str>>, fps: f64) -> Self {
        Self {
            path: path.into(),
            fps,
        }
    }
}

impl From<&str> for MediaCopy {
    /// A copy whose rate isn't known (the decoder pool probes it).
    fn from(path: &str) -> Self {
        Self::new(path, 0.0)
    }
}

impl AsRef<str> for MediaCopy {
    fn as_ref(&self) -> &str {
        &self.path
    }
}

/// Each source path (as the project names it) with an editing copy, and that copy. Only the preview's context has one;
/// export builds its context without, so it always reads the originals.
pub type MediaMap = HashMap<Arc<str>, MediaCopy>;

/// Data shared by every frame of one project revision.
#[derive(Clone)]
pub struct PlanContext {
    pub project: Arc<Project>,
    /// Ids, paths and texts as shared strings (kept across revisions of one project).
    pub strings: Arc<Strings>,
    /// The zoom springs and camera runs, precomputed per change of what they read (only the runs that changed).
    pub zoom: Arc<zoom::ZoomTrack>,
    /// Where each main clip starts, and the main track's length (frames find their clip by binary search).
    pub starts: Arc<[Us]>,
    pub main_duration_us: Us,
    /// How long the video is: the main track or its last picture layer, whichever ends later.
    /// Playback's end and seeks stop here; past the main track a frame is the background with the layers on top.
    pub video_duration_us: Us,
    /// Where each transition runs, in timeline order (built once per revision: `transitions::windows` allocates).
    pub transitions: Arc<[TransitionWindow]>,
    /// What contributions draw: the built-ins, packs and shader plugins. The export worker builds the same one.
    pub registry: Arc<Registry>,
    /// Which tracks the solo buttons leave on (resolved once per revision, never per frame).
    pub solo: Solo,
    /// The editing copies the preview reads instead of their sources (`with_media`; empty for export).
    pub media: Arc<MediaMap>,
    /// The recordings' cursor takes: the session's cache, loaded in the background, or export's,
    /// loaded before its first frame. A context without them zooms Auto on the centre and crops at the focus.
    pub cursors: Arc<CursorTracks>,
    /// The takes of `cursors` it plans with, and their follow paths, as they were when it was made (`takes`).
    pub takes: Arc<Takes>,
    pub follows: Arc<Follows>,
    /// The caption pages of this revision, per format (`with_captions`; none until set, and a context made from a
    /// previous one keeps its pages until the feed sets new ones). Built once per revision (the engine's feed, from its
    /// speech; a dry run only for the previewed format) or frozen in an export's job, never per frame.
    pub captions: Arc<Captions>,
    /// The captions' resolved style and per-format look: made once per change of the caption settings or the
    /// formats; a context made from a previous one keeps it while they stay.
    pub caption_style: Arc<CaptionLook>,
    /// The language the project's video text is in (`klipolo_model::text_language`): the Han characters of a text without
    /// kana or Hangul take its face. The feed sets it (`with_text_language`; English until then), an export
    /// freezes it in its job, and a context made from a previous one keeps it.
    pub text_language: Arc<str>,
}

/// The text language a context starts with until the feed or a job says otherwise.
const DEFAULT_TEXT_LANGUAGE: &str = "en";

impl PlanContext {
    pub fn new(project: Arc<Project>) -> Self {
        Self::with_registry(project, Registry::builtin())
    }

    /// A context drawing contributions from `registry` (export builds the preview's), its Auto zooms on the centre.
    pub fn with_registry(project: Arc<Project>, registry: Arc<Registry>) -> Self {
        Self::with_cursors(project, registry, Arc::default())
    }

    /// A context whose Auto zooms and follow-crops follow `cursors`' takes (export loads its project's first:
    /// `media::cursor::wanted`), every zoom made now.
    pub fn with_cursors(
        project: Arc<Project>,
        registry: Arc<Registry>,
        cursors: Arc<CursorTracks>,
    ) -> Self {
        let (starts, takes) = Self::read(&project, &cursors);
        let (_, build) = zoom::ZoomBuild::of(project.clone(), starts.clone(), takes.clone(), None);
        let zoom = Arc::new(build.finish());
        let made = (Arc::default(), Arc::default(), Arc::default());
        Self::with(
            project,
            registry,
            zoom,
            made,
            cursors,
            (starts, takes, None),
            None,
        )
    }

    /// The clip starts and the takes the plan follows now.
    fn read(project: &Project, cursors: &CursorTracks) -> (Arc<[Us]>, Arc<Takes>) {
        let starts = klipolo_model::query::clip_starts(&project.timeline.main).into();
        (starts, Arc::new(Takes::of(project, cursors)))
    }

    /// `look`: the previous context's caption look, kept when this project's captions and formats are the same.
    fn with(
        project: Arc<Project>,
        registry: Arc<Registry>,
        zoom: Arc<zoom::ZoomTrack>,
        (strings, media, captions): (Arc<Strings>, Arc<MediaMap>, Arc<Captions>),
        cursors: Arc<CursorTracks>,
        (starts, takes, follows): (Arc<[Us]>, Arc<Takes>, Option<Arc<Follows>>),
        look: Option<&Arc<CaptionLook>>,
    ) -> Self {
        let caption_style = CaptionLook::keep_or_make(look, &project);
        let follows = follows.unwrap_or_else(|| Arc::new(takes.follows()));
        let main_duration_us = klipolo_model::query::main_track_duration(&project.timeline.main);
        let video_duration_us = klipolo_model::query::video_duration(&project.timeline);
        let transitions = klipolo_model::transitions::windows(&project.timeline.main).into();
        let solo = Solo::of(&project.timeline.tracks);
        Self {
            project,
            strings,
            zoom,
            starts,
            main_duration_us,
            video_duration_us,
            transitions,
            registry,
            solo,
            media,
            cursors,
            takes,
            follows,
            captions,
            caption_style,
            text_language: Arc::from(DEFAULT_TEXT_LANGUAGE),
        }
    }

    /// This context reading sources through `media`'s editing copies (the preview's; a new revision keeps them).
    pub fn with_media(mut self, media: Arc<MediaMap>) -> Self {
        self.media = media;
        self
    }

    /// This context drawing `captions` (the engine's feed builds them per revision from its `Speech`; export reads its
    /// job's `captions.json`).
    pub fn with_captions(mut self, captions: Arc<Captions>) -> Self {
        self.captions = captions;
        self
    }

    /// This context drawing CJK in `language`'s faces (the feed's `text_language`, or an export job's).
    pub fn with_text_language(mut self, language: Arc<str>) -> Self {
        self.text_language = language;
        self
    }

    /// The transition running at `at_us`, if any (windows don't overlap; binary search).
    pub fn transition_at(&self, at_us: Us) -> Option<&TransitionWindow> {
        let k = self.transitions.partition_point(|w| w.start_us <= at_us);
        k.checked_sub(1)
            .map(|k| &self.transitions[k])
            .filter(|w| at_us < w.end_us)
    }

    /// The shader `w`'s transition draws with, or None when this build doesn't have it (its frames are then a cut).
    pub fn transition_shader(&self, w: &TransitionWindow) -> Option<&Arc<ShaderEffect>> {
        let t = self.project.timeline.main[w.clip].transition_out.as_ref()?;
        self.registry
            .shader(&t.effect)
            .filter(|sh| sh.kind == ShaderKind::Transition)
    }

    /// Index and start of the main clip at `at_us` (`klipolo_model::query::clip_at` by binary search).
    pub fn clip_at(&self, at_us: Us) -> Option<(usize, Us)> {
        let i = self
            .starts
            .partition_point(|&s| s <= at_us)
            .checked_sub(1)?;
        let clip = &self.project.timeline.main[i];
        (at_us < self.starts[i] + clip.duration_us()).then(|| (i, self.starts[i]))
    }

    /// A context for an edited project that keeps `previous`'s zoom springs when no zoom item and not the
    /// main track's length changed (drag previews arrive up to 60 times a second), and otherwise simulates only the
    /// zoom runs that changed. It keeps `previous`'s cursor tracks and captions; an edit that can move words sets new
    /// captions (`with_captions`).
    pub fn reusing(project: Arc<Project>, previous: &PlanContext) -> Self {
        match Self::starting(project, Some(previous), &previous.cursors) {
            (ctx, None) => ctx,
            (mut ctx, Some(build)) => {
                ctx.zoom = Arc::new(build.finish());
                ctx
            }
        }
    }

    /// Like `reusing`, but a rebuild bigger than `zoom::INLINE_STEPS` comes back as a `ZoomBuild` to finish off the
    /// caller's thread; meanwhile the context zooms only where it could reuse runs, or where an earlier plan covers the
    /// same span (the actor swaps the finished track in with `PlanContext::with_zoom` and draws again). Auto zooms and
    /// follow-crops follow `cursors` (the session's cache): a take that has loaded, or whose activity grew, since
    /// `previous` changes what its camera runs read, so they are planned again. Its captions are `previous`'s (none
    /// without one) until the caller sets new ones (`with_captions`).
    pub fn starting(
        project: Arc<Project>,
        previous: Option<&PlanContext>,
        cursors: &Arc<CursorTracks>,
    ) -> (Self, Option<zoom::ZoomBuild>) {
        let read = Self::read(&project, cursors);
        Self::start(project, previous, cursors, (read.0, read.1, None))
    }

    /// `starting` for an edit while the video plays: with `previous`'s takes and follow paths as they were when it was
    /// made, so a path or plan the loader built since never swaps in mid-play. The session plans
    /// with the newest at the next pause.
    pub fn starting_held(
        project: Arc<Project>,
        previous: &PlanContext,
        cursors: &Arc<CursorTracks>,
    ) -> (Self, Option<zoom::ZoomBuild>) {
        let starts = klipolo_model::query::clip_starts(&project.timeline.main).into();
        let read = (
            starts,
            previous.takes.clone(),
            Some(previous.follows.clone()),
        );
        Self::start(project, Some(previous), cursors, read)
    }

    fn start(
        project: Arc<Project>,
        previous: Option<&PlanContext>,
        cursors: &Arc<CursorTracks>,
        (starts, takes, follows): (Arc<[Us]>, Arc<Takes>, Option<Arc<Follows>>),
    ) -> (Self, Option<zoom::ZoomBuild>) {
        let strings = previous.map_or_else(Arc::default, |p| p.strings.clone());
        let registry = previous.map_or_else(Registry::builtin, |p| p.registry.clone());
        let media = previous.map_or_else(Arc::default, |p| p.media.clone());
        let captions = previous.map_or_else(Arc::default, |p| p.captions.clone());
        let language = previous.map(|p| p.text_language.clone());
        let look = previous.map(|p| &p.caption_style);
        let cursors = cursors.clone();
        let read = (starts.clone(), takes.clone(), follows);
        let (target, build) =
            zoom::ZoomBuild::of(project.clone(), starts, takes, previous.map(|p| &*p.zoom));
        let (zoom, build) = match previous {
            Some(previous) if previous.zoom.matches(&target) => (previous.zoom.clone(), None),
            _ if build.steps() <= zoom::INLINE_STEPS => (Arc::new(build.finish()), None),
            _ => (Arc::new(build.partial()), Some(build)),
        };
        let made = (strings, media, captions);
        let ctx = Self::with(project, registry, zoom, made, cursors, read, look);
        (ctx.keeping_language(language), build)
    }

    /// This context in the previous one's text language, when there was one.
    fn keeping_language(self, language: Option<Arc<str>>) -> Self {
        match language {
            Some(language) => self.with_text_language(language),
            None => self,
        }
    }

    /// Takes a finished zoom track if it was built for this context's zoom items. False: it is stale.
    pub fn with_zoom(&mut self, track: zoom::ZoomTrack) -> bool {
        if !track.same_items(&self.zoom) {
            return false;
        }
        self.zoom = Arc::new(track);
        true
    }
}
