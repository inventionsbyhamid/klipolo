//! Auto zooms at real scale (the Algorithms rule), in release: an hour-long one, its plan and glide and a frame's window
//! over it; and two hours cut into 1,500 clips, planned off the actor, with an edit elsewhere reusing the plan.

use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use klipolo_cursor::CursorTrack;
use klipolo_engine::media::cursor::load_track;
use klipolo_engine::plan::PlanContext;

use super::support;
use super::support::recording::auto_zoom_project;
use super::support::secs;

/// `seconds` of a tutorial: a click every 3–7 s somewhere new, the cursor gliding there between clicks (arriving 0.4 s
/// before, leaving 0.1 s after), a sample every 33 ms. With its clicks' count.
fn tutorial(seconds: f64) -> (CursorTrack, usize) {
    let (mut keys, mut clicks) = (Vec::new(), Vec::new());
    let (mut t, mut k) = (2.0, 0_u32);
    while t < seconds - 10.0 {
        let x = 0.1 + 0.8 * ((k * 37 % 100) as f32 / 100.0);
        let y = 0.1 + 0.8 * ((k * 61 % 100) as f32 / 100.0);
        keys.extend([(t - 0.4, x, y), (t + 0.1, x, y)]);
        clicks.push((t, x, y));
        t += 3.0 + f64::from(k % 5);
        k += 1;
    }
    let f = tempfile::NamedTempFile::new().unwrap();
    let mut w = std::io::BufWriter::new(f.as_file());
    for i in 0..(seconds as i64 * 30) {
        let s = i as f64 / 30.0;
        let j = keys.partition_point(|k: &(f64, f32, f32)| k.0 <= s);
        let (x, y) = match (j.checked_sub(1).map(|j| keys[j]), keys.get(j)) {
            (Some(a), Some(b)) => {
                let u = ((s - a.0) / (b.0 - a.0)) as f32;
                (a.1 + (b.1 - a.1) * u, a.2 + (b.2 - a.2) * u)
            }
            (Some(a), None) => (a.1, a.2),
            (None, Some(b)) => (b.1, b.2),
            (None, None) => (0.5, 0.5),
        };
        writeln!(w, r#"{{"k":"m","t":{},"x":{x},"y":{y}}}"#, i * 33_333).unwrap();
    }
    for &(c, x, y) in &clicks {
        for (t, d) in [(secs(c), 1), (secs(c) + 50_000, 0)] {
            writeln!(w, r#"{{"k":"b","t":{t},"btn":0,"d":{d},"x":{x},"y":{y}}}"#).unwrap();
        }
    }
    w.flush().unwrap();
    drop(w);
    (load_track(f.path()).unwrap(), clicks.len())
}

/// `zoom.window` at 60 Hz over `seconds`: how long it took and how many frames were zoomed.
fn windows(
    zoom: &klipolo_engine::plan::zoom::ZoomTrack,
    seconds: f64,
) -> (std::time::Duration, usize) {
    let started = Instant::now();
    let mut zoomed = 0;
    for t in (0..secs(seconds)).step_by(16_667) {
        zoomed += usize::from(zoom.window(t).is_some());
    }
    (started.elapsed(), zoomed)
}

#[test]
#[ignore = "real scale: an hour-long Auto zoom, in release"]
fn an_hour_long_auto_zoom_plans_in_under_100_ms() {
    // The hour of 7,200 steps, as a plan and its glide: one Auto zoom over the whole hour.
    let (track, clicks) = tutorial(3_600.0);
    let (mut p, tracks) = auto_zoom_project(track, 0.0, 3_600.0, 2.0);
    support::lengthen(&mut p, 3_600.0);
    p.timeline.main[0].source_out_us = secs(3_600.0);
    let p = Arc::new(p);
    let started = Instant::now();
    let zoom = klipolo_engine::plan::zoom::ZoomTrack::new(&p, &tracks);
    let took = started.elapsed();
    let run = &zoom.cameras()[0].1;
    let (sampled, zoomed) = windows(&zoom, 3_600.0);
    println!(
        "an hour-long Auto zoom over {clicks} clicks: planned with its glide in {took:?} ({} shots, {} KB); 216,000 \
         windows in {sampled:?}, {zoomed} zoomed",
        run.plan.shots().len(),
        run.plan.bytes() / 1024
    );
    assert!(run.plan.shots().len() > 300, "{}", run.plan.shots().len());
    assert!(took.as_millis() < 100, "planned in {took:?}");
}

#[test]
#[ignore = "real scale: two hours in 1,500 clips under one Auto zoom, in release"]
fn two_hours_in_1500_clips_plan_off_the_actor_and_an_edit_elsewhere_keeps_the_plan() {
    let (track, clicks) = tutorial(7_200.0);
    let (mut p, tracks) = auto_zoom_project(track, 0.0, 7_200.0, 2.0);
    support::lengthen(&mut p, 7_200.0);
    // The take cut every 4.8 s, played end to end.
    let first = p.timeline.main[0].clone();
    p.timeline.main = (0..1_500)
        .map(|i| {
            let mut c = first.clone();
            c.id = format!("c{i}");
            (c.source_in_us, c.source_out_us) = (i * 4_800_000, (i + 1) * 4_800_000);
            c
        })
        .collect();
    let (p, cursors) = (Arc::new(p), Arc::new(tracks));
    // On the actor: the context and its keys; the plan comes back as a build to finish off it.
    let started = Instant::now();
    let (mut ctx, build) = PlanContext::starting(p.clone(), None, &cursors);
    let listed = started.elapsed();
    let build = build.expect("over INLINE_STEPS: planned off the actor");
    let started = Instant::now();
    let track = build.finish();
    let planned = started.elapsed();
    assert!(ctx.with_zoom(track));
    // An edit that changes nothing the camera reads (the project renamed, as a drag of a text layer would): the plan
    // is kept, on the actor.
    let mut edited = (*p).clone();
    edited.name = "edited".into();
    let started = Instant::now();
    let (next, build) = PlanContext::starting(Arc::new(edited), Some(&ctx), &cursors);
    let rekeyed = started.elapsed();
    assert!(build.is_none() && Arc::ptr_eq(&next.zoom, &ctx.zoom));
    let run = &next.zoom.cameras()[0].1;
    let (sampled, zoomed) = windows(&next.zoom, 7_200.0);
    println!(
        "two hours, 1,500 clips, {clicks} clicks: listed on the actor in {listed:?}; planned with its glide off it in \
         {planned:?} ({} shots, {} KB); an edit elsewhere kept it in {rekeyed:?}; 432,000 windows in {sampled:?}, \
         {zoomed} zoomed",
        run.plan.shots().len(),
        run.plan.bytes() / 1024
    );
    assert!(planned.as_millis() < 200, "planned in {planned:?}");
    assert!(rekeyed.as_millis() < 5, "kept in {rekeyed:?}");
}
