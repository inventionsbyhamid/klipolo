//! The part typed lately. A typed line longer than the crop is followed where it is typed. Playing media inside the
//! typing box is never followed, nor is a second place changing beside the text. Typing inside playing media counts
//! once a click lands there.

mod common;

use common::activity::{activity_of, cells, tick_of};
use common::follow::{S, alone, at, click, cursor_of, glide, rest, span, vertical};
use klipolo_cursor::activity::{Region, Tick, typing};
use klipolo_cursor::facts::StillRuns;
use klipolo_cursor::follow::{Crop, FollowInput, FollowPath};
use klipolo_cursor::format::Line;
use klipolo_cursor::{CursorTrack, RegionSpan, Us};

/// A line typed across most of the picture: grid cells 4–59 of 64 on rows 18–19, one every 0.3 s (a cell holds about 3
/// characters: a fast typist), with `beside(k)` changing too where it gives a region.
fn line_typed(beside: impl Fn(u8) -> Option<Region>) -> Vec<Tick> {
    let tick = |k: u8| {
        let typed = (k < 168 && k.is_multiple_of(3)).then(|| cells(4 + k / 3, 18, 1, 2));
        tick_of(&[typed, beside(k)].into_iter().flatten().collect::<Vec<_>>())
    };
    (0..200).map(tick).collect()
}

/// The cursor resting on the left for 20 s.
fn resting_left() -> CursorTrack {
    let mut lines = Vec::new();
    rest(&mut lines, (0, 20 * S), (0.1, 0.5));
    cursor_of(lines)
}

/// A paragraph's typing box over the line, from 0 to 17 s: cells 4–59 across, rows 14–21.
fn paragraph() -> [RegionSpan; 1] {
    [span(0, 17 * S, of(cells(4, 14, 56, 8)))]
}

/// Cells as a part of the picture.
fn of(r: Region) -> (f32, f32, f32, f32) {
    let r = r.rect();
    (r.x, r.y, r.w, r.h)
}

/// The newest typed cell is inside the crop at 4, 8, 12 and 15 s.
fn follows_the_text(p: &FollowPath, crop: Crop) {
    for t in [4 * S, 8 * S, 12 * S, 15 * S] {
        shows(p, crop, t, 4 + (t / 300_000) as u8);
    }
}

/// Grid column `cell` is inside the crop at `t`.
fn shows(p: &FollowPath, crop: Crop, t: Us, cell: u8) {
    let (x0, x1) = (f64::from(cell) / 64.0, f64::from(cell + 1) / 64.0);
    let (cx, _) = at(p, t);
    let shown = cx - crop.w / 2.0 <= x0 && x1 <= cx + crop.w / 2.0;
    assert!(
        shown,
        "column {cell} ({x0}–{x1}) is outside the crop at {t}: {cx}"
    );
}

#[test]
fn a_long_typed_line_is_followed_where_it_is_typed_not_at_its_middle() {
    // The line alone, while the cursor rests on the left: its span's box (the activity track's) is wider than the crop
    // and than a quarter of the picture.
    let crop = vertical(1.0);
    let track = resting_left();
    let a = activity_of(&line_typed(|_| None));
    let typing = typing(&a, &StillRuns::of(&track), 0);
    assert_eq!(typing.len(), 1, "{typing:?}");
    assert!(typing[0].rect.w > 0.8, "{typing:?}");
    let runs = [(0, &a)];
    let input = FollowInput {
        typing: &typing,
        activity: &runs,
        ..alone(&track, crop)
    };
    follows_the_text(&FollowPath::build(&input), crop);
    // Without the ticks it can't know where the newest text is: it stays with the cursor, not the line's middle.
    let p = FollowPath::build(&FollowInput {
        activity: &[],
        ..input
    });
    assert!(at(&p, 15 * S).0 < 0.2, "{:?}", at(&p, 15 * S));
}

#[test]
fn playing_media_inside_a_large_typing_box_is_never_followed() {
    // An animation inside the paragraph's box plays from the start (a GIF: 4 × 2 cells changing every tick, larger
    // than a keystroke, so first in each tick), and the activity track found it playing.
    let crop = vertical(1.0);
    let track = resting_left();
    let gif = cells(30, 20, 4, 2);
    let a = activity_of(&line_typed(|_| Some(gif)));
    let (typing, media, runs) = (paragraph(), [span(0, 20 * S, of(gif))], [(0, &a)]);
    let input = FollowInput {
        typing: &typing,
        media: &media,
        activity: &runs,
        ..alone(&track, crop)
    };
    follows_the_text(&FollowPath::build(&input), crop);
}

#[test]
fn a_second_place_changing_beside_the_text_never_pulls_the_crop_off_it() {
    // A live preview below the line changes with it from 2 s to 11 s: 3 cells, larger than a keystroke, so first in
    // each tick, and not playing media. The crop keeps to the text it follows.
    let crop = vertical(1.0);
    let track = resting_left();
    let preview = cells(44, 21, 3, 1);
    let a = activity_of(&line_typed(|k| (20..110).contains(&k).then_some(preview)));
    let (typing, runs) = (paragraph(), [(0, &a)]);
    let input = FollowInput {
        typing: &typing,
        activity: &runs,
        ..alone(&track, crop)
    };
    follows_the_text(&FollowPath::build(&input), crop);
}

#[test]
fn typing_inside_playing_media_counts_once_a_click_lands_there() {
    // A large box reads as playing media from 1 s to 12 s (say a page that kept scrolling). The person clicks a field
    // inside it at 4.5 s, moves the mouse away to the left, and types in the field from 5 s to 9 s.
    let crop = vertical(1.0);
    let at_8_5 = |clicked: bool| {
        let mut lines = Vec::new();
        rest(&mut lines, (0, 4_400_000), (0.5, 0.5));
        lines.push(Line::Move {
            t: 4_400_000,
            x: 0.5,
            y: 0.5,
        });
        glide(&mut lines, (4_400_000, 4_500_000), (0.5, 0.5), (0.86, 0.31));
        if clicked {
            click(&mut lines, 4_500_000, (0.86, 0.31));
        }
        glide(&mut lines, (4_600_000, 4_900_000), (0.86, 0.31), (0.2, 0.5));
        rest(&mut lines, (5 * S, 12 * S), (0.2, 0.5));
        let track = cursor_of(lines);
        let media = [span(S, 12 * S, (0.55, 0.15, 0.4, 0.4))];
        let typing = [span(5 * S, 9 * S, (0.85, 0.3, 0.05, 0.03))];
        let input = FollowInput {
            typing: &typing,
            media: &media,
            ..alone(&track, crop)
        };
        at(&FollowPath::build(&input), 8_500_000).0
    };
    assert!(
        at_8_5(true) > 0.7,
        "the typing is followed: {}",
        at_8_5(true)
    );
    assert!(
        at_8_5(false) < 0.4,
        "unclicked, the media's typing isn't: {}",
        at_8_5(false)
    );
}

#[test]
fn a_jump_to_a_new_line_is_followed_though_a_counter_keeps_changing_by_the_old_one() {
    // A short line typed on the left until 3.3 s, with a live counter inside it (2 cells changing every tick, so it
    // never grows the part); from 5 s the typing goes on in a line far to the right. Once
    // nothing has grown the part for 1 s, the crop follows the new line.
    let crop = vertical(1.0);
    let (p, cell_at) = jump_with_a_counter(crop, 6);
    for t in [9 * S, 12 * S] {
        shows(&p, crop, t, cell_at(t));
    }
}

#[test]
fn slow_typing_keeps_the_crop_on_its_line_though_a_counter_changes_far_off() {
    // The same, with the new line typed slowly (a cell every 1.2 s; the keystrokes between change the caret's cell
    // without growing the part): the changes there keep the part fresh, so the crop never flaps over to the counter
    // by the old line.
    let crop = vertical(1.0);
    let (p, cell_at) = jump_with_a_counter(crop, 12);
    for t in (14..34).map(|half| half * S / 2) {
        shows(&p, crop, t, cell_at(t));
    }
}

/// The far-line take above with the new line typed a cell every `per_cell` ticks: its follow path, and the new line's
/// newest column at a time from 5 s on.
fn jump_with_a_counter(crop: Crop, per_cell: u8) -> (FollowPath, impl Fn(Us) -> u8) {
    let track = resting_left();
    let tick = |k: u8| {
        let old = (k < 36 && k.is_multiple_of(3)).then(|| cells(4 + k / 3, 18, 1, 2));
        let new = (50..170).contains(&k) && k.is_multiple_of(3);
        let new = new.then(|| cells(38 + (k - 50) / per_cell, 26, 1, 2));
        tick_of(
            &[old, new, Some(cells(8, 18, 2, 1))]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
        )
    };
    let a = activity_of(&(0..200).map(tick).collect::<Vec<_>>());
    let (typing, runs) = ([span(0, 17 * S, of(cells(2, 14, 60, 16)))], [(0, &a)]);
    let input = FollowInput {
        typing: &typing,
        activity: &runs,
        ..alone(&track, crop)
    };
    let cell_at = move |t: Us| 38 + ((t / 100_000) as u8 / 3 * 3 - 50) / per_cell;
    (FollowPath::build(&input), cell_at)
}
