//! While playing, an edit keeps the takes and follow paths as they were, so a path the loader built mid-play never
//! swaps in at the edit; the pause takes it. Both ways an edit reaches the session: a committed revision (`on_feed`)
//! and a preview's dry run (`refresh`).

use std::time::Duration;

use klipolo_cursor::format::{Capture, Line};
use klipolo_model::Op;
use klipolo_model::cursor_style::{CursorStyle, Follow};
use klipolo_model::layout::BoxN;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::timeline::{Framing, FramingMode};

use super::support::{self, secs, videos};
use super::{Harness, eventually, harness, last, publish};
use klipolo_engine::api::ViewSpec;

/// The portrait crop of the latest frame at or after `after`.
fn crop_at(h: &Harness, after: i64) -> Option<BoxN> {
    last(h)
        .filter(|r| r.plan.at_us >= after && r.plan.format == FormatKind::Portrait)
        .map(|r| videos(&r.plan)[0].crop)
}

/// The crop follows the cursor, which rests at x 0.8 from half a second in.
fn follows(c: BoxN) -> bool {
    (c.x + c.w / 2.0 - 0.8).abs() < 0.07
}

/// A portrait take whose cursor rests at the right, paused at 10 s with the still following it, then playing; then
/// following 1.5× closer is published while it plays (a new path the loader builds now), and 800 ms pass so it has.
async fn playing_with_a_closer_path_built(h: &Harness, dir: &std::path::Path) -> Project {
    h.engine.set_view(ViewSpec {
        format: FormatKind::Portrait,
        width: 36,
        height: 64,
    });
    let mut lines = vec![support::takes::header(Capture::Burned)];
    lines.extend((0..=120).map(|k| Line::Move {
        t: k * 500_000,
        x: if k == 0 { 0.5 } else { 0.8 },
        y: 0.5,
    }));
    let mut p = support::recording::cursor_project(dir, &lines, CursorStyle::default());
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    publish(h, "p1", 1, p.clone());
    h.engine.seek(secs(10.0), 1);
    eventually(|| crop_at(h, secs(10.0)).is_some_and(follows)).await;
    h.engine.play(secs(10.0), 1.0, 0).await.unwrap();
    p.timeline.main[0].framing.insert(
        FormatKind::Portrait,
        Framing {
            mode: FramingMode::Fill,
            focus_x: 0.5,
            focus_y: 0.5,
            follow: Some(Follow { closer: 1.5 }),
        },
    );
    publish(h, "p1", 2, p.clone());
    tokio::time::sleep(Duration::from_millis(800)).await;
    p
}

/// Still playing after a second edit: the closer crop at its focus, never the path that landed mid-play; the pause
/// takes it.
async fn held_until_the_pause(h: &Harness) {
    let rendered = h.log.lock().unwrap().len();
    eventually(|| h.log.lock().unwrap().len() > rendered + 5).await;
    let c = crop_at(h, secs(10.0)).unwrap();
    assert!((c.w - 0.5625 / (16.0 / 9.0) / 1.5).abs() < 1e-6, "{c:?}");
    assert!((c.x + c.w / 2.0 - 0.5).abs() < 1e-6, "held: {c:?}");
    let at = h.engine.pause().await;
    eventually(|| crop_at(h, at).is_some_and(follows)).await;
}

#[tokio::test]
async fn an_edit_while_playing_keeps_the_follow_paths_until_the_pause() {
    let h = harness().await;
    let dir = tempfile::tempdir().unwrap();
    let mut p = playing_with_a_closer_path_built(&h, dir.path()).await;
    p.name = "renamed".into();
    publish(&h, "p1", 3, p);
    held_until_the_pause(&h).await;
}

#[tokio::test]
async fn a_preview_while_playing_keeps_the_follow_paths_until_the_pause() {
    let h = harness().await;
    let dir = tempfile::tempdir().unwrap();
    playing_with_a_closer_path_built(&h, dir.path()).await;
    // A drag's dry run against revision 2 (`refresh`), not a new revision.
    let rename = Op::Rename {
        name: "renamed".into(),
    };
    h.engine.preview("p1".into(), 2, vec![rename]);
    held_until_the_pause(&h).await;
}
