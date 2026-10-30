//! The project feed, the UI's preview ops and the background work they start: zoom springs too long to simulate on
//! the actor, assets the backend loads in the background, and the cursor tracks Auto zooms follow.
//! Caption pages are built from the feed's speech for every format once per revision that may page differently (and per
//! new speech); a dry run that can move words makes its previewed format's only and keeps the others'. Where
//! someone speaks, which ducking sound layers go down under, is worked out from it once per revision (and per new
//! speech when something ducks).

use std::sync::Arc;

use klipolo_model::apply_ops;
use klipolo_model::captions::{captions, previewed};
use tokio::task::JoinHandle;

use super::{Actor, Current};
use crate::api::{EngineEvent, StateReason};
use crate::audio::plan::{AudioPlan, audio_plan, ducking_speech, ducks, may_change_sound};
use crate::media;
use crate::plan::PlanContext;
use crate::plan::captions::{may_move_captions, pages_may_differ};
use crate::plan::zoom::{ZoomBuild, ZoomFor, ZoomTrack};
use crate::session::{PreviewBackend, ProjectFeed};

/// Zoom springs being simulated on a blocking thread, and what they're for.
pub(crate) struct ZoomJob {
    target: ZoomFor,
    handle: JoinHandle<ZoomTrack>,
}

/// The zoom build's result; never resolves without one. Cancel-safe.
pub(super) async fn zoom_done(job: &mut Option<ZoomJob>) -> Option<ZoomTrack> {
    match job {
        Some(job) => (&mut job.handle).await.ok(),
        None => std::future::pending().await,
    }
}

/// The takes' screen activity read from `store`, and its "a track grew" signal; none without a store.
pub(super) fn read_activity(
    cursors: &crate::media::cursor::CursorTracks,
    store: Option<Arc<media::activity::ActivityStore>>,
) -> Option<Arc<tokio::sync::Notify>> {
    let store = store?;
    let wake = store.wake();
    cursors.set_activity(Arc::new(move |path: &str| store.get(path)));
    Some(wake)
}

/// Resolves when the backend's signal fires; never without one.
pub(super) async fn redrawn(signal: &Option<Arc<tokio::sync::Notify>>) {
    match signal {
        Some(signal) => signal.notified().await,
        None => std::future::pending().await,
    }
}

/// The sound of feed `f`, the same revision as `c`: `c`'s, unless new speech moves where one of its sound layers ducks;
/// then planned again (and still `c`'s when that changed nothing, so nothing that holds it starts over).
fn same_sound(c: &Current, f: &ProjectFeed) -> Arc<AudioPlan> {
    if Arc::ptr_eq(&c.speech, &f.speech) || !ducks(&f.project) {
        return c.sound.clone();
    }
    let sound = audio_plan(
        &f.project,
        &f.registry,
        ducking_speech(&f.project, &f.speech),
    );
    if sound == *c.sound {
        c.sound.clone()
    } else {
        Arc::new(sound)
    }
}

impl<B: PreviewBackend> Actor<B> {
    pub(super) fn on_feed(&mut self) {
        let feed = self.p.feed.borrow_and_update().clone();
        let switched = match (&feed, &self.current) {
            (Some(f), Some(c)) => f.project_id != c.project_id || f.opened != c.opened,
            (None, None) => false,
            _ => true,
        };
        let changed = match (&feed, &self.current) {
            (Some(f), Some(c)) => switched || f.revision != c.revision,
            (None, None) => false,
            _ => true,
        };
        if changed {
            // Preview ops were made against the previous revision; they expire with it.
            self.transient = None;
        }
        if switched {
            // A new project starts paused at 0 (the UI resets its playhead to 0 as well). A play still waiting for the
            // preview's size says it stopped, so nothing that heard it start (the app's busy registry) waits on it.
            self.cancel_queued_play(StateReason::Pause, Some(0));
            self.stop_playing(0, StateReason::Pause);
            if self.current.is_some() {
                // Its decoded audio goes (builds cancelled); cached files stay.
                self.p.backend.close_audio();
            }
            // Its loudness measurement stops, and its gain never carries over.
            self.level.reset();
            self.p.backend.set_master_gain(1.0);
            // Also the "no project open" path: the engine stays up but lets decoders and textures go.
            self.session = self.session.wrapping_add(1);
            // Frames of the previous project are dropped with its textures.
            self.inflight.clear();
            self.p.backend.reset();
            if feed.is_none() {
                self.presenter.release();
            }
            self.playhead_us = 0;
            self.prepared = None;
            // A text typed on the canvas belongs to the opening it was typed in.
            self.editing = None;
            // The previous project's cursor tracks go with it (a load under way is dropped).
            self.cursors.clear();
            self.warned.clear();
            self.missing_said.clear();
            let note = self.waiting.clear();
            self.note(note);
        }
        let previous = self.current.take();
        let mut build = None;
        // The same revision of the same project sent again with a new speech (a transcript finished), text language (the
        // interface's changed) or registry (the app's snapshot checked an effect): its context, zoom springs
        // (a build still running included) and sound plan are kept, and only the pages, the language or what draws are
        // made again.
        let mut kept = false;
        let media = self.media.clone();
        let cursors = self.cursors.clone();
        let playing = self.playing.is_some();
        self.current = feed.map(|f| {
            let same = previous.as_ref().filter(|c| {
                !switched && c.revision == f.revision && Arc::ptr_eq(&c.committed, &f.project)
            });
            // Pages once per speech and revision that may page differently (an edit that moves no word keeps them).
            let kept_pages = previous.as_ref().filter(|c| {
                !switched
                    && Arc::ptr_eq(&c.speech, &f.speech)
                    && (Arc::ptr_eq(&c.committed, &f.project)
                        || !pages_may_differ(&c.committed, &f.project))
            });
            let captions = match kept_pages {
                Some(c) => c.captions.clone(),
                None => Arc::new(captions(&f.project, &f.speech)),
            };
            let (ctx, sound) = match same {
                // Its committed context: a dry run's is planned again from it by `refresh`.
                Some(c) if Arc::ptr_eq(&c.ctx.project, &f.project) => {
                    kept = true;
                    (c.ctx.clone(), same_sound(c, &f))
                }
                _ => {
                    // An edit keeps the zoom springs that didn't change; while playing, the takes and follow paths
                    // too.
                    let reuse = previous.as_ref().filter(|_| !switched).map(|c| &c.ctx);
                    let (ctx, zoom) = match reuse.filter(|_| playing) {
                        Some(held) => PlanContext::starting_held(f.project.clone(), held, &cursors),
                        None => PlanContext::starting(f.project.clone(), reuse, &cursors),
                    };
                    build = zoom;
                    // Planned once for the revision: the cache, the loudness and playback all take this one.
                    let sound = match same {
                        Some(c) => same_sound(c, &f),
                        None => {
                            let speech = ducking_speech(&f.project, &f.speech);
                            Arc::new(audio_plan(&f.project, &ctx.registry, speech))
                        }
                    };
                    (ctx, sound)
                }
            };
            // What draws is the feed's: a new registry for the same revision draws the next frame with it.
            let mut ctx = ctx;
            if !Arc::ptr_eq(&ctx.registry, &f.registry) {
                ctx.registry = f.registry.clone();
            }
            // The editing copies are the app's, for whatever project is open (keyed by source path).
            let ctx = ctx
                .with_media(media)
                .with_captions(captions.clone())
                .with_text_language(f.text_language.clone());
            Current {
                ctx,
                project_id: f.project_id,
                revision: f.revision,
                opened: f.opened,
                committed: f.project,
                speech: f.speech,
                captions,
                sound,
                quiet: true,
            }
        });
        if !kept {
            self.build_zoom(build);
            // What a held context passed over is planned at the pause.
            self.cursor_due |= playing && !switched;
        }
        if switched && self.current.is_some() {
            self.emit(EngineEvent::Ready {
                session: self.session,
            });
        }
        if let Some(cur) = &self.current {
            self.p
                .backend
                .prepare_audio(cur.sound.clone(), self.playhead_us);
            self.p
                .backend
                .prepare_media(media::wants(&cur.committed, self.playhead_us));
            // What draws reached the engine: its agent-made shaders compile now, as they are placed, and only
            // its pipelines stay.
            if !self
                .prepared
                .as_ref()
                .is_some_and(|r| Arc::ptr_eq(r, &cur.ctx.registry))
            {
                self.prepared = Some(cur.ctx.registry.clone());
                self.p.backend.prepare_registry(&cur.ctx.registry);
            }
            self.level.want(cur.sound.clone());
            // Its cursor takes, read and smoothed off the actor in timeline order (a file that couldn't be read is
            // tried again): until one arrives no cursor is drawn over it and its zooms look at the centre.
            self.cursors.want(&media::cursor::wanted(&cur.committed));
            // Its screen files' activity, read or analysed in the background after the thumbnails.
            if let Some(activity) = self.p.backend.activity() {
                activity.want(&media::activity::wanted(&cur.committed));
            }
        }
        self.refresh();
    }

    /// Use the dry run of the UI's preview ops when they were made against exactly this revision; a newer revision
    /// expires them, so committing a drag never flickers.
    pub(super) fn on_transient(&mut self) {
        self.transient = self.p.transient.borrow_and_update().clone();
        self.refresh();
    }

    pub(super) fn refresh(&mut self) {
        let Some(cur) = self.current.as_mut() else {
            return;
        };
        // A dry run whose ops can't change the sound sounds like the committed revision (`Current::sound`); one whose
        // ops can't move a word or a page shows the committed pages.
        let (project, quiet, moved) = match &self.transient {
            Some(t) if t.project_id == cur.project_id && t.base_revision == cur.revision => {
                match apply_ops(&cur.committed, &t.ops) {
                    Ok(project) => (
                        Arc::new(project),
                        !t.ops.iter().any(|op| may_change_sound(&cur.committed, op)),
                        t.ops.iter().any(|op| may_move_captions(&cur.committed, op)),
                    ),
                    Err(_) => (cur.committed.clone(), true, false),
                }
            }
            _ => (cur.committed.clone(), true, false),
        };
        cur.quiet = quiet;
        if !Arc::ptr_eq(&project, &cur.ctx.project) {
            // A drag that moves words draws them where they go at once, before any commit: the previewed format's pages
            // only.
            let captions = match (moved, self.view) {
                (false, _) => cur.captions.clone(),
                (true, Some(view)) => {
                    Arc::new(previewed(&project, &cur.speech, view.format, &cur.captions))
                }
                (true, None) => Arc::new(captions(&project, &cur.speech)),
            };
            // While playing, the takes and follow paths stay as they were; the pause plans anew.
            let (ctx, build) = match self.playing.is_some() {
                true => PlanContext::starting_held(project, &cur.ctx, &cur.ctx.cursors),
                false => PlanContext::starting(project, Some(&cur.ctx), &cur.ctx.cursors),
            };
            self.cursor_due |= self.playing.is_some();
            cur.ctx = ctx.with_captions(captions);
            self.build_zoom(build);
        }
        self.dirty = true;
        self.replan_audio();
    }

    /// Simulates zoom springs too long for the actor on a blocking thread. A running build for the same zoom items
    /// keeps going (a commit right after its preview needs exactly that track); a build for other items replaces it.
    /// Without a build the current context's track is complete, so a running one is stale.
    fn build_zoom(&mut self, build: Option<ZoomBuild>) {
        if let (Some(build), Some(job)) = (&build, &self.zoom_job)
            && build.targets(&job.target)
        {
            return;
        }
        if let Some(job) = self.zoom_job.take() {
            job.handle.abort();
        }
        self.zoom_job = build.map(|b| ZoomJob {
            target: b.target(),
            handle: tokio::task::spawn_blocking(move || b.finish()),
        });
    }

    /// A cursor take loaded or was built again (or couldn't be read): the Auto zooms over it are planned again and its
    /// follow paths taken; runs and plans that didn't change are kept, and a long rebuild runs off the actor as an
    /// edit's does. A take the cursor layer draws (or its new smoothing) changes the frame too, so it is drawn again
    /// While playing, a new plan or path would jump: it waits for the next pause (`cursor_due`); the cursor
    /// layer draws the take as it is meanwhile.
    pub(super) fn on_cursor(&mut self) {
        if self.playing.is_some() {
            self.cursor_due = true;
            self.dirty |= self
                .current
                .as_ref()
                .is_some_and(|c| media::cursor::draws_any(&c.ctx.project));
            return;
        }
        self.cursor_due = false;
        let Some(cur) = self.current.as_mut() else {
            return;
        };
        let (ctx, build) =
            PlanContext::starting(cur.ctx.project.clone(), Some(&cur.ctx), &cur.ctx.cursors);
        let changed = build.is_some() || !Arc::ptr_eq(&ctx.zoom, &cur.ctx.zoom);
        let drawn = media::cursor::draws_any(&ctx.project);
        // A follow path that landed changes a following crop, whatever the takes' capture.
        let followed = !ctx.follows.same(&cur.ctx.follows);
        cur.ctx = ctx;
        if changed {
            self.build_zoom(build);
        }
        self.dirty |= changed || drawn || followed;
    }

    /// A screen file's activity grew (a chunk analysed, or a file read from the cache): the takes it belongs to
    /// build their facts and follow paths again for it, in the background; `on_cursor` plans with them.
    pub(super) fn on_activity(&mut self) {
        if let Some(cur) = &self.current {
            self.cursors.want(&media::cursor::wanted(&cur.committed));
        }
    }

    /// A zoom build finished: use it if it is for the current zoom items, and draw again.
    pub(super) fn on_zoom(&mut self, track: Option<ZoomTrack>) {
        self.zoom_job = None;
        if let (Some(track), Some(cur)) = (track, self.current.as_mut())
            && cur.ctx.with_zoom(track)
        {
            self.dirty = true;
        }
    }
}
