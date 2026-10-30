//! Export plans what the preview plans: the screens' activity its takes' facts and follow
//! paths read is made before the first frame, so its camera runs and follow crops equal the preview's over the whole
//! timeline, also at the budget's edge, where the takes' sizes (their activity's facts included) decide which are held
//!. A screen that can't be analysed is planned without its activity by both: export marks it in the
//! folder they share, and the editor reads the mark.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use klipolo_engine::export::prepare::{
    Analysed, Analysis, analyse, load_cursors, load_cursors_into, load_cursors_with, screen_files,
};
use klipolo_engine::media::activity::{self, ActivityStore};
use klipolo_engine::media::cursor::{CursorTracks, wanted};
use klipolo_engine::media::store::ThumbStore;
use klipolo_engine::plan::{PlanContext, frame_plan};
use klipolo_engine::registry::Registry;
use klipolo_model::cursor_style::CursorCapture;
use klipolo_model::project::FormatKind::{Landscape, Portrait};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::timeline::{ItemContent, ZoomTarget};
use klipolo_platform::durable::Os;

use crate::support::{bx, item, zoom_track};
use crate::typing::{LEN, opener};
use crate::{NOW, WAIT, add_take, click, glide, preview_ctx, preview_cursors, rest, write_take};

/// Three burned-in screen takes of 60 s, back to back, in landscape and portrait (which follows), under one Auto zoom:
/// each clicks at 2 s, rests while a line is typed (20–35 s), and moves on at 40 s to click again. Their screens are
/// stand-in files in `dir` (the decoder is synthetic); the third can't be analysed.
fn three_takes(dir: &Path) -> (Project, Vec<String>) {
    let mut p = Project::new("Typing");
    p.id = "project".into();
    p.settings.fps = crate::FPS;
    p.settings.formats = vec![Landscape, Portrait];
    let mut screens = Vec::new();
    for k in 1..=3 {
        let mut lines: Vec<_> = glide((0.5, 0.5), (0.3, 0.6), 0.0, 2.0).collect();
        lines.extend(click((0.3, 0.6), 2.0, 2.1));
        lines.extend(rest(0.3, 0.6, 2.0, 40.0));
        lines.extend(glide((0.3, 0.6), (0.7, 0.3), 40.0, 42.0));
        lines.extend(click((0.7, 0.3), 42.0, 42.1));
        lines.extend(rest(0.7, 0.3, 42.0, 60.0));
        let take = write_take(dir, k, &[], lines);
        let screen = dir.join(format!("screen{k}.mov"));
        std::fs::write(&screen, b"footage").unwrap();
        let screen = screen.to_string_lossy().into_owned();
        add_take(
            &mut p,
            k,
            &screen,
            60.0,
            (&take, CursorCapture::Burned),
            (0.0, 60.0),
        );
        screens.push(screen);
    }
    let auto = ItemContent::Zoom {
        target: ZoomTarget::Auto { using: vec![] },
        scale: 2.0,
    };
    p.timeline.tracks = vec![zoom_track(vec![item(
        "auto",
        1.0,
        178.0,
        bx(0.0, 0.0, 1.0, 1.0),
        auto,
    )])];
    (p, screens)
}

/// The preview's takes over `root`'s activity (the editor's store, analysing in the background as it does), within
/// `budget`, waited for.
fn preview(p: &Project, store: &Arc<ActivityStore>, budget: usize) -> PlanContext {
    let tracks = CursorTracks::with_budget(budget);
    let store = store.clone();
    tracks.set_activity(Arc::new(move |path: &str| store.get(path)));
    preview_ctx(p, &preview_cursors(p, tracks))
}

/// Export's context within `budget`, with the activity it analysed.
fn exported(p: &Project, analysed: &Analysed, budget: usize) -> PlanContext {
    let tracks = CursorTracks::with_budget(budget);
    tracks.set_activity(analysed.clone().source());
    let loaded = load_cursors_into(tracks, p);
    PlanContext::with_cursors(Arc::new(p.clone()), Registry::builtin(), loaded.cursors)
}

/// The cursor file of take `k`.
fn take_of(p: &Project, k: usize) -> String {
    p.recordings[k - 1].cursor.as_ref().unwrap().path.clone()
}

/// Each take's activity fingerprint as `ctx` plans with it (None: not held).
fn prints(p: &Project, ctx: &PlanContext) -> Vec<Option<u64>> {
    (1..=3)
        .map(|k| ctx.takes.get(&take_of(p, k)).map(|t| t.activity()))
        .collect()
}

/// `a` and `b` plan the same camera runs, zoom windows and pictures (landscape and portrait) over 600 frames.
fn same_plans(a: &PlanContext, b: &PlanContext) {
    let keys = |c: &PlanContext| {
        c.zoom
            .cameras()
            .iter()
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(a), keys(b), "the camera runs' keys");
    assert!(!keys(a).is_empty());
    let end = a.video_duration_us;
    for n in 0..600 {
        let t = end * n / 600;
        assert_eq!(a.zoom.window(t), b.zoom.window(t), "the zoom at {t} µs");
        for format in [Landscape, Portrait] {
            assert_eq!(
                frame_plan(a, t, format),
                frame_plan(b, t, format),
                "{format:?} at {t} µs"
            );
        }
    }
}

#[test]
fn export_plans_the_same_zooms_and_crops_as_preview() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("activity");
    let (p, screens) = three_takes(dir.path());
    assert_eq!(
        screen_files(&p, &wanted(&p)),
        screens.iter().map(|s| (s.clone(), LEN)).collect::<Vec<_>>(),
        "every take is under the Auto zoom"
    );
    // Export first, before the editor has analysed anything: it makes each screen's activity, and marks the third,
    // which can't be analysed.
    let opens = Arc::new(AtomicUsize::new(0));
    let export_store =
        ActivityStore::detached(root.clone(), opener(screens[2].clone(), opens.clone()));
    let never = Arc::new(AtomicBool::new(false));
    let files = screen_files(&p, &wanted(&p));
    let Ok(Analysis::Done(analysed)) = analyse(&export_store, &files, &never, &|_| {}, NOW) else {
        panic!("analysed");
    };
    assert_eq!(analysed.len(), 2, "the third can't be analysed");
    assert!(analysed.get(&screens[0]).is_some_and(|t| t.is_complete()));
    // The editor's store, in the same folder, reads what export made (nothing decoded again) and the third's mark.
    let thumbs = ThumbStore::new(dir.path().join("thumbs"), Arc::new(Os));
    let seen = Arc::new(AtomicUsize::new(0));
    let store = ActivityStore::attach_with(
        &thumbs,
        root.clone(),
        opener(screens[2].clone(), seen.clone()),
    );
    store.want(&activity::wanted(&p));
    let deadline = Instant::now() + WAIT;
    while store.pending()
        || !screens[..2]
            .iter()
            .all(|s| store.get(s).is_some_and(|t| t.is_complete()))
    {
        assert!(
            Instant::now() < deadline,
            "the editor's store reads the screens"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(seen.load(Ordering::SeqCst), 0, "nothing analysed twice");
    assert!(store.get(&screens[2]).is_none());

    // Every take held: the same activity, facts, camera runs and crops.
    let budget = klipolo_engine::media::cursor::BUDGET_BYTES;
    let (ours, theirs) = (exported(&p, &analysed, budget), preview(&p, &store, budget));
    assert_eq!(prints(&p, &ours), prints(&p, &theirs));
    let typed = |ctx: &PlanContext, k| {
        let take = ctx.takes.get(&take_of(&p, k)).unwrap();
        take.facts().typing.as_ref().map_or(0, Vec::len)
    };
    assert!(
        typed(&ours, 1) > 0 && typed(&ours, 2) > 0,
        "the typing is known"
    );
    assert_eq!(typed(&ours, 3), 0, "the third is planned without activity");
    same_plans(&ours, &theirs);
    // As export planned before (no activity), its runs differ: this is what the test guards.
    let before = load_cursors(&p).cursors;
    let without = PlanContext::with_cursors(Arc::new(p.clone()), Registry::builtin(), before);
    assert_ne!(prints(&p, &without), prints(&p, &theirs));
    // The real path: `load_cursors_with` the activity, at the engine's budget.
    let real = load_cursors_with(&p, &wanted(&p), Some(analysed.clone().source())).cursors;
    let real = PlanContext::with_cursors(Arc::new(p.clone()), Registry::builtin(), real);
    same_plans(&real, &theirs);

    // At the budget's edge: room for the first two takes as the preview holds them, a byte short of the
    // third. Export holds the same two, so it plans the same; without their activity's facts the takes are smaller,
    // and the third would fit as well.
    let bytes = |k| theirs.cursors.get(&take_of(&p, k)).unwrap().bytes();
    for k in 1..=3 {
        assert_eq!(
            ours.cursors.get(&take_of(&p, k)).unwrap().bytes(),
            bytes(k),
            "take {k}'s size"
        );
    }
    let edge = bytes(1) + bytes(2) + bytes(3) - 1;
    let (ours, theirs) = (exported(&p, &analysed, edge), preview(&p, &store, edge));
    assert_eq!(prints(&p, &theirs)[2], None, "the third is past the edge");
    assert_eq!(prints(&p, &ours), prints(&p, &theirs));
    same_plans(&ours, &theirs);
    let tracks = CursorTracks::with_budget(edge);
    let without = load_cursors_into(tracks, &p).cursors;
    let without = PlanContext::with_cursors(Arc::new(p.clone()), Registry::builtin(), without);
    assert!(
        prints(&p, &without)[2].is_some(),
        "without activity the third fits"
    );
}

/// A take's plans read its screens' activity only under an Auto zoom, or following (`Want::plans_read_activity`): a
/// landscape-only take with no Auto zoom reads none, so neither export nor the preview analyses its screens for its plans,
/// and its facts and size don't depend on how far their analysis has got. Its screens stay listed (chat's facts read
/// facts read every take's).
#[test]
fn only_the_takes_whose_plans_read_activity_have_it_analysed() {
    let dir = tempfile::tempdir().unwrap();
    let (mut p, _) = three_takes(dir.path());
    p.timeline.tracks.clear();
    let reads = |p: &Project| {
        wanted(p)
            .iter()
            .map(|w| w.plans_read_activity)
            .collect::<Vec<_>>()
    };
    assert_eq!(reads(&p), [true; 3], "portrait follows");
    p.settings.formats = vec![FormatKind::Landscape];
    assert_eq!(reads(&p), [false; 3]);
    assert!(
        wanted(&p).iter().all(|w| w.screens.len() == 1),
        "every screen still listed"
    );
    assert!(screen_files(&p, &wanted(&p)).is_empty());
    // An Auto zoom over the second take's clip, touching the third's start.
    p.timeline.tracks = vec![zoom_track(vec![item(
        "auto",
        70.0,
        50.0,
        bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Zoom {
            target: ZoomTarget::Auto { using: vec![] },
            scale: 2.0,
        },
    )])];
    assert_eq!(reads(&p), [false, true, true]);
}
