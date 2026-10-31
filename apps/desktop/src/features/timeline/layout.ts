/** The timeline's fixed sizes (px) and the empty time after the video (µs). */

/** The id of the hint every block points screen readers to (`aria-describedby`): how to move between blocks. */
export const BLOCK_HINT_ID = 'timeline-block-keys';

/** The gutter on the left that holds each lane's icon (`--timeline-gutter`). */
export const HEADER_PX = 40;
/** The ruler's height: proposed cuts and the time range start just under it. */
export const RULER_PX = 24;
/**
 * The Video lane takes the timeline's spare height; with two or three overlay rows in a short window that is under
 * 44 px, but never less than this (app.css's `.tl-row.is-grow`). jsdom (no layout) draws it at the
 * default.
 */
export const MAIN_LANE_MIN_PX = 24;
export const MAIN_LANE_PX = 68;
/** A clip's inside, where its filmstrip and waveform draw: the lane less its 1 px edges. */
export const CLIP_INNER_PX = MAIN_LANE_PX - 2;
/** The share of a clip's height its filmstrip takes; its own waveform fills the rest, along the bottom. */
export const FILM_SHARE = 0.62;
/** One packed row of the Overlays lane (items are 18 px in a 20 px row), and the padding above and below its rows. */
export const LAYER_ROW_PX = 20;
export const LAYERS_PAD_PX = 4;
/** The Sounds and Music lanes; in a short window (under 820 px tall, `SHORT_WINDOW`) 20 and 28. */
export const SOUNDS_LANE_PX = 22;
export const MUSIC_LANE_PX = 34;
export const SHORT_SOUNDS_LANE_PX = 20;
export const SHORT_MUSIC_LANE_PX = 28;
export const SHORT_WINDOW = '(max-height: 819px)';
/**
 * The narrowest each block draws: a clip's edges and padding; an overlay's edge, icon and padding; a sound effect's or
 * music's 6 px (an effect is a teal block at least 6 px wide). Blocks narrower than that merge into a band
 * in their lane's colour: drawn one by one they would only overlap, and an hour at mid zoom would mount every clip.
 */
export const CLIP_MIN_PX = 16;
export const LAYER_MIN_PX = 12;
export const SOUND_MIN_PX = 6;
/**
 * A bare cut's marker (Add transition, or the note that none fits) draws only between clips at least this wide, unless
 * it has focus: a window then mounts at most 3 views / 40 px, about 90 of them, inside the mounted-block budget.
 */
export const CUT_MIN_PX = 40;
/**
 * A transition's chip draws between clips at least this wide (or when focused or selected); between narrower clips
 * (but not banded ones) it shows as a tint at the clips' edges (`ClipBlock`), no element of its own.
 */
export const CHIP_MIN_PX = 30;
/** Room after the video to drop and drag into. */
export const TAIL_US = 10_000_000;
