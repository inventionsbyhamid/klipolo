//! A main clip's footage for tracking (an imported video, or a camera take's run), the formats whose Fill crops it,
//! its path from a tracked stretch (in its own source time, trimmed to what it plays), and the op that makes it follow.

use std::collections::BTreeMap;

use klipolo_analysis::providers::{Heard, TrackedBox, TrackedFrame};
use klipolo_analysis::subject::clips::{
    Speech, camera_voice, clip_path, clip_people, cropped_formats, follow_op, follow_op_keeping,
    followable_formats, footage, speaker_path, voice_windows,
};
use klipolo_analysis::subject::voices::{SoundBeside, VoiceTurn, turns_beside};
use klipolo_analysis::subject::{FPS, PERSON, SubjectTrack};
use klipolo_model::crop::Crop;
use klipolo_model::fill_focus::{FillFocus, FocusKey, FocusPath, Keep, Person, Voice};
use klipolo_model::ops::{ClipPatch, Op};
use klipolo_model::project::FormatKind::{Landscape, Portrait, Square};
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::recording::{
    Recording, RecordingKind, RecordingSource, SourceMix, SourceRole, SourceRun,
};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource, Framing, FramingMode};

fn s(seconds: f64) -> Us {
    (seconds * 1e6) as Us
}

fn video(id: &str, w: u32, h: u32) -> Asset {
    Asset {
        id: id.into(),
        kind: AssetKind::Video,
        path: format!("/media/{id}.mp4"),
        name: format!("{id}.mp4"),
        duration_us: s(600.0),
        width: w,
        height: h,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }
}

fn source(id: &str, role: SourceRole, offset_us: Us, file_in_us: Us) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: format!("capture.source.{id}"),
        runs: vec![SourceRun {
            asset_id: id.into(),
            offset_us,
            duration_us: s(300.0),
            drift_ppm: 0.0,
            file_in_us,
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    }
}

/// A talk (an imported 16:9 video) cut to 30 to 40 s, a camera take whose camera starts 2 s in, and a screen take.
fn project() -> Project {
    let mut p = Project::new("Talk");
    p.assets = vec![
        video("talk", 1920, 1080),
        video("cam", 1280, 720),
        video("screen", 1920, 1080),
    ];
    p.settings.formats = vec![Landscape, Portrait, Square];
    p.recordings = vec![
        Recording {
            id: "camtake".into(),
            kind: RecordingKind::Camera,
            sources: vec![source("cam", SourceRole::Camera, s(2.0), s(1.0))],
            primary: "cam".into(),
            cursor: None,
            markers: vec![],
            game: None,
        },
        Recording {
            id: "screentake".into(),
            kind: RecordingKind::Screen,
            sources: vec![source("screen", SourceRole::Screen, 0, 0)],
            primary: "screen".into(),
            cursor: None,
            markers: vec![],
            game: None,
        },
    ];
    let clip = |id: &str, source: ClipSource, from: f64, to: f64| {
        let mut c = Clip::new(source, s(from), s(to));
        c.id = id.into();
        c
    };
    p.timeline.main = vec![
        clip("talk1", ClipSource::Asset { id: "talk".into() }, 30.0, 40.0),
        clip(
            "cam1",
            ClipSource::Recording {
                id: "camtake".into(),
            },
            12.0,
            20.0,
        ),
        clip(
            "screen1",
            ClipSource::Recording {
                id: "screentake".into(),
            },
            0.0,
            5.0,
        ),
    ];
    p
}

/// One person at `x` on every frame of a stretch.
fn track(range: RangeUs, x: f64) -> SubjectTrack {
    let frames = ((range.end - range.start) as f64 / 1e6 * FPS) as u32;
    SubjectTrack {
        model: "sam-3.1".into(),
        phrase: PERSON.into(),
        fps: FPS,
        range,
        frames: (0..frames)
            .map(|frame| TrackedFrame {
                frame,
                boxes: vec![TrackedBox {
                    id: 1,
                    left: x - 0.1,
                    top: 0.1,
                    right: x + 0.1,
                    bottom: 1.0,
                    confidence: Some(0.9),
                }],
            })
            .collect(),
        cuts: vec![],
        frames_billed: u64::from(frames),
        cost_usd: 0.0,
    }
}

#[test]
fn an_imported_video_and_a_camera_take_have_footage_and_a_screen_take_does_not() {
    let p = project();
    let talk = footage(&p, &p.timeline.main[0]).unwrap();
    assert_eq!(
        (talk.asset_id.as_str(), talk.path.as_str()),
        ("talk", "/media/talk.mp4")
    );
    assert_eq!(
        (talk.range, talk.shift_us),
        (
            RangeUs {
                start: s(30.0),
                end: s(40.0)
            },
            0
        )
    );
    // The camera run starts 2 s into the take and 1 s into its file: take time 12 s is file time 11 s.
    let cam = footage(&p, &p.timeline.main[1]).unwrap();
    assert_eq!(cam.asset_id, "cam");
    assert_eq!(
        (cam.range, cam.shift_us),
        (
            RangeUs {
                start: s(11.0),
                end: s(19.0)
            },
            s(1.0)
        )
    );
    // A screen take follows the cursor instead.
    assert!(footage(&p, &p.timeline.main[2]).is_none());
}

/// A "Record yourself" take lands as one clip from the take's 0, and its camera's first frame comes a frame after the
/// start gate: the clip's footage is that run, from its first frame on, not nothing.
#[test]
fn a_camera_take_whose_camera_starts_a_frame_late_still_has_footage() {
    let mut p = project();
    p.recordings[0].sources[0].runs[0].offset_us = 33_000;
    p.recordings[0].sources[0].runs[0].file_in_us = 0;
    p.timeline.main[1].source_in_us = 0;
    let cam = footage(&p, &p.timeline.main[1]).unwrap();
    assert_eq!(cam.asset_id, "cam");
    assert_eq!(
        (cam.range, cam.shift_us),
        (
            RangeUs {
                start: 0,
                end: s(20.0) - 33_000
            },
            33_000
        )
    );
    // A clip that ends before the camera's first frame has none.
    p.timeline.main[1].source_out_us = 20_000;
    assert!(footage(&p, &p.timeline.main[1]).is_none());
}

/// A camera take's camera has no sound of its own: the voice beside it is its mic's, lined up on the take's clock. The
/// camera starts 2 s into the take and 1 s into its file, the mic at the take's start: the mic's 2 s is the camera's
/// 1 s, so the mic's turns move 1 s earlier on the camera's file.
#[test]
fn a_camera_take_s_voice_is_its_mic_lined_up_beside_the_camera() {
    let mut p = project();
    let mut mic = video("mic", 0, 0);
    mic.kind = AssetKind::Audio;
    p.assets.push(mic);
    p.recordings[0]
        .sources
        .push(source("mic", SourceRole::Mic, 0, 0));
    let beside = camera_voice(&p, "cam");
    assert_eq!(
        beside,
        [SoundBeside {
            asset_id: "mic".into(),
            path: "/media/mic.mp4".into(),
            span: RangeUs {
                start: s(1.0),
                end: s(299.0)
            },
            lead_us: -s(1.0),
        }]
    );
    let turn = |from: f64, to: f64| VoiceTurn {
        start_us: s(from),
        end_us: s(to),
        voice: Voice {
            part: 0,
            label: "A".into(),
        },
    };
    let turns = turns_beside(&beside, |id| {
        assert_eq!(id, "mic");
        vec![turn(1.0, 5.0), turn(10.0, 20.0), turn(299.5, 302.0)]
    });
    // Cut to where the camera's file and the mic both play: from the camera's first frame to the mic's end.
    assert_eq!(
        turns,
        [turn(1.0, 4.0), turn(9.0, 19.0), turn(298.5, 299.0)]
    );
    // An imported video, a screen take and a camera take without a mic have none.
    for file in ["talk", "screen"] {
        assert!(camera_voice(&p, file).is_empty(), "{file}");
    }
    assert!(camera_voice(&project(), "cam").is_empty());
}

#[test]
fn fill_crops_a_wide_video_only_in_vertical_and_square() {
    let p = project();
    let all = [Landscape, Portrait, Square];
    assert_eq!(
        cropped_formats(&p, &p.timeline.main[0], &all),
        [Portrait, Square]
    );
}

#[test]
fn a_clips_path_is_in_its_source_time_and_trimmed_to_what_it_plays() {
    let p = project();
    // A stretch of the talk wider than the clip: 20 to 50 s.
    let wide = track(
        RangeUs {
            start: s(20.0),
            end: s(50.0),
        },
        0.3,
    );
    let clip = &p.timeline.main[0];
    let path = clip_path(
        &p,
        clip,
        Portrait,
        &footage(&p, clip).unwrap(),
        std::slice::from_ref(&wide),
    )
    .unwrap();
    assert!(path.is_valid());
    let first = path.keys.first().unwrap();
    let last = path.keys.last().unwrap();
    assert!(
        first.at_us >= s(29.5) && first.at_us <= s(30.0),
        "{first:?}"
    );
    assert!(last.at_us >= s(39.75) && last.at_us <= s(40.5), "{last:?}");
    assert!(!first.cut);
    assert!((first.x - 0.3).abs() < 0.02);

    // The camera clip's keys are on the take's clock: the file's 11 s is the take's 12 s.
    let clip = &p.timeline.main[1];
    let cam = footage(&p, clip).unwrap();
    let path = clip_path(&p, clip, Portrait, &cam, &[track(cam.range, 0.6)]).unwrap();
    assert!(
        path.keys.first().unwrap().at_us >= s(11.5),
        "{:?}",
        path.keys[0]
    );
    assert!(path.keys.last().unwrap().at_us <= s(20.5));
    // Nobody tracked: no path.
    let empty = SubjectTrack {
        frames: vec![],
        ..track(cam.range, 0.5)
    };
    assert!(clip_path(&p, clip, Portrait, &cam, &[empty]).is_none());
}

#[test]
fn the_follow_op_fills_each_format_with_its_path_and_keeps_the_rest() {
    let mut p = project();
    let own = Framing {
        mode: FramingMode::Fit,
        focus_x: 0.2,
        focus_y: 0.5,
        follow: None,
    };
    p.timeline.main[0].framing.insert(Portrait, own);
    p.timeline.main[0]
        .fill_focus
        .insert(Landscape, FillFocus::Center);
    let path = FocusPath {
        keys: vec![FocusKey {
            at_us: s(30.0),
            x: 0.3,
            y: 0.5,
            cut: false,
        }],
    };
    let op = follow_op(
        &p,
        &p.timeline.main[0],
        &BTreeMap::from([(Portrait, path.clone())]),
    );
    let Op::UpdateClip { id, patch } = op else {
        panic!("{op:?}")
    };
    assert_eq!(id, "talk1");
    let ClipPatch {
        framing: Some(framing),
        fill_focus: Some(focus),
        ..
    } = patch
    else {
        panic!()
    };
    // Portrait fills now, its own focus point kept for Fixed; the other formats as they were.
    assert_eq!(
        framing[&Portrait],
        Framing {
            mode: FramingMode::Fill,
            ..own
        }
    );
    assert!(!framing.contains_key(&Square));
    assert_eq!(focus[&Portrait], FillFocus::speaker(path));
    assert_eq!(focus[&Landscape], FillFocus::Center);
}

#[test]
fn a_clip_covered_by_two_tracked_stretches_reads_both() {
    let p = project();
    let clip = &p.timeline.main[0];
    // The talk's 30 to 40 s: tracked as 20 to 35 s (the speaker at 0.3), then 35 to 50 s (at 0.6).
    let early = track(
        RangeUs {
            start: s(20.0),
            end: s(35.0),
        },
        0.3,
    );
    let late = track(
        RangeUs {
            start: s(35.0),
            end: s(50.0),
        },
        0.6,
    );
    let path = clip_path(
        &p,
        clip,
        Portrait,
        &footage(&p, clip).unwrap(),
        &[late, early],
    )
    .unwrap();
    assert!((path.centre_at(s(31.0)).unwrap().0 - 0.3).abs() < 0.02);
    assert!(path.centre_at(s(39.5)).unwrap().0 > 0.4, "{path:?}");
}

#[test]
fn a_cropped_clips_path_sizes_its_crop_in_the_whole_file() {
    let mut p = project();
    // The left half cut off: Fill in 9:16 shows the same share of the file's width as uncropped, not of what is left.
    p.timeline.main[0].crop = Some(Crop {
        left: 0.5,
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
    });
    let clip = &p.timeline.main[0];
    let near_edge = track(
        RangeUs {
            start: s(30.0),
            end: s(40.0),
        },
        0.8,
    );
    let path = clip_path(
        &p,
        clip,
        Portrait,
        &footage(&p, clip).unwrap(),
        &[near_edge],
    )
    .unwrap();
    assert!(
        (path.centre_at(s(35.0)).unwrap().0 - 0.8).abs() < 0.02,
        "{path:?}"
    );
}

#[test]
fn a_crop_that_keeps_one_guest_follows_that_guest() {
    let mut p = project();
    // A two-person interview cropped to its right half: the bigger guest on the left is cut off.
    p.timeline.main[0].crop = Some(Crop {
        left: 0.5,
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
    });
    let clip = &p.timeline.main[0];
    let guest = |id: u32, cx: f64, width: f64| TrackedBox {
        id,
        left: cx - width / 2.0,
        top: 0.15,
        right: cx + width / 2.0,
        bottom: 1.0,
        confidence: Some(0.9),
    };
    let range = RangeUs {
        start: s(30.0),
        end: s(40.0),
    };
    let both = SubjectTrack {
        frames: (0..20)
            .map(|frame| TrackedFrame {
                frame,
                boxes: vec![guest(1, 0.3, 0.3), guest(2, 0.75, 0.15)],
            })
            .collect(),
        ..track(range, 0.5)
    };
    let path = clip_path(&p, clip, Portrait, &footage(&p, clip).unwrap(), &[both]).unwrap();
    for at in [30.0, 35.0, 39.5] {
        let x = path.centre_at(s(at)).unwrap().0;
        assert!((x - 0.75).abs() < 0.02, "at {at} s: {x} in {path:?}");
    }
}

#[test]
fn a_format_the_person_set_to_fit_is_never_followed() {
    let mut p = project();
    let all = [Landscape, Portrait, Square];
    let fit = Framing {
        mode: FramingMode::Fit,
        focus_x: 0.5,
        focus_y: 0.5,
        follow: None,
    };
    p.timeline.main[0].framing.insert(Square, fit);
    // Square crops the picture when it fills, but it doesn't fill: only the vertical version follows.
    assert_eq!(
        cropped_formats(&p, &p.timeline.main[0], &all),
        [Portrait, Square]
    );
    assert_eq!(
        followable_formats(&p, &p.timeline.main[0], &all),
        [Portrait]
    );
}

fn voice(label: &str) -> Voice {
    Voice {
        part: 0,
        label: label.into(),
    }
}

fn turn(from: f64, to: f64, label: &str) -> VoiceTurn {
    VoiceTurn {
        start_us: s(from),
        end_us: s(to),
        voice: voice(label),
    }
}

/// Two people far apart on every frame of a stretch: tracked as 7 on the left and 3 on the right.
fn two(range: RangeUs) -> SubjectTrack {
    let mut t = track(range, 0.25);
    for f in &mut t.frames {
        let left = f.boxes[0];
        f.boxes = vec![
            TrackedBox {
                id: 3,
                left: 0.65,
                right: 0.85,
                ..left
            },
            TrackedBox { id: 7, ..left },
        ];
    }
    t
}

/// The talk's clip plays 30 to 40 s: A (on the left) speaks to 35 s, then B (on the right); C is off camera.
fn interview() -> (Vec<VoiceTurn>, BTreeMap<Voice, Heard>) {
    let turns = vec![
        turn(30.0, 35.0, "A"),
        turn(35.0, 39.0, "B"),
        turn(39.0, 40.0, "C"),
    ];
    let heard = BTreeMap::from([
        (voice("A"), Heard::Person { person: 1 }),
        (voice("B"), Heard::Person { person: 2 }),
        (voice("C"), Heard::Nobody),
    ]);
    (turns, heard)
}

#[test]
fn a_two_person_clip_follows_whoever_speaks_in_its_own_time() {
    let p = project();
    let clip = &p.timeline.main[0];
    let foot = footage(&p, clip).unwrap();
    let tracks = [two(RangeUs {
        start: s(20.0),
        end: s(50.0),
    })];
    let (turns, heard) = interview();
    let speech = Speech {
        turns: &turns,
        voices: &heard,
        keep: &Keep::Speaker,
    };
    let path = speaker_path(&p, clip, Portrait, &foot, &tracks, &speech).unwrap();
    let x = |at: f64| path.centre_at(s(at)).unwrap().0;
    assert!((x(32.0) - 0.25).abs() < 0.03, "{path:?}");
    assert!((x(37.0) - 0.75).abs() < 0.03, "{path:?}");
    assert!(
        path.keys.iter().any(|k| k.cut && k.at_us == s(35.0)),
        "{path:?}"
    );
    // Without voices it frames the main subject, as before.
    assert_eq!(
        speaker_path(&p, clip, Portrait, &foot, &tracks, &Speech::none()),
        clip_path(&p, clip, Portrait, &foot, &tracks)
    );
}

#[test]
fn a_clips_people_are_named_left_to_right_with_their_voices_and_share() {
    let p = project();
    let clip = &p.timeline.main[0];
    let foot = footage(&p, clip).unwrap();
    let tracks = [two(foot.range)];
    let (turns, heard) = interview();
    let people = clip_people(clip, &foot, &tracks, &turns, &heard);
    assert_eq!(
        people,
        [
            Person {
                voices: vec![voice("A")],
                share: 50,
            },
            Person {
                voices: vec![voice("B")],
                share: 40,
            },
        ]
    );
    // One person: nobody to choose between.
    assert!(clip_people(clip, &foot, &[track(foot.range, 0.4)], &turns, &heard).is_empty());
}

#[test]
fn a_clips_windows_are_only_in_the_footage_it_plays() {
    let p = project();
    let clip = &p.timeline.main[0];
    let foot = footage(&p, clip).unwrap();
    let tracks = [two(RangeUs {
        start: s(20.0),
        end: s(50.0),
    })];
    let turns = [turn(20.0, 33.0, "A"), turn(33.0, 50.0, "B")];
    let windows = voice_windows(&p, clip, &[Portrait, Square], &foot, &tracks, &turns);
    assert!(!windows.is_empty());
    assert!(
        (windows.iter()).all(|w| w.range.start >= s(30.0) && w.range.end <= s(40.0)),
        "{windows:?}"
    );
}

#[test]
fn the_follow_op_keeps_who_it_frames_and_the_people() {
    let p = project();
    let clip = &p.timeline.main[0];
    let path = FocusPath {
        keys: vec![FocusKey {
            at_us: s(30.0),
            x: 0.3,
            y: 0.5,
            cut: false,
        }],
    };
    let people = vec![Person::default(), Person::default()];
    let keep = Keep::Person { person: 2 };
    let op = follow_op_keeping(
        &p,
        clip,
        &BTreeMap::from([(Portrait, path.clone())]),
        &keep,
        &people,
    );
    let Op::UpdateClip { patch, .. } = op else {
        panic!("{op:?}")
    };
    assert_eq!(
        patch.fill_focus.unwrap()[&Portrait],
        FillFocus::Speaker { path, keep, people }
    );
}
