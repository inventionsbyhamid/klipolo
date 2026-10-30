/** The timeline's fixed sizes (px) and the empty time after the video (µs). */

/** The id of the hint every block points screen readers to (`aria-describedby`): how to move between blocks. */
export const BLOCK_HINT_ID = 'timeline-block-keys';

/** The gutter on the left that holds each lane's icon (`--timeline-gutter`). */
export const HEADER_PX = 40;
/** The ruler's height: proposed cuts and the time range start just under it. */
export const RULER_PX = 24;
/**
 * The Video lane takes the timeline's spare height, never less than this; jsdom (no layout) draws it at the default.
 */
export const MAIN_LANE_MIN_PX = 44;
export const MAIN_LANE_PX = 68;
/** A clip's inside, where its filmstrip and waveform draw: the lane less its 1 px edges. */
export const CLIP_INNER_PX = MAIN_LANE_PX - 2;
/** The share of a clip's height its filmstrip takes; its own waveform fills the rest, along the bottom. */
export const FILM_SHARE = 0.62;
/** One packed row of the Overlays lane (items are 18 px in a 20 px row), and the padding above and below its rows. */
export const LAYER_ROW_PX = 20;
export const LAYERS_PAD_PX = 4;
/** The Sounds and Music lanes. */
export const SOUNDS_LANE_PX = 22;
export const MUSIC_LANE_PX = 34;
/**
 * The narrowest each block draws: a clip's edges and padding; a layer's edge and padding, rounded up. Blocks narrower
 * than that merge into a band: drawn one by one they would only overlap, and an hour at mid zoom would mount every
 * clip.
 */
export const CLIP_MIN_PX = 16;
export const LAYER_MIN_PX = 12;
/** Room after the video to drop and drag into. */
export const TAIL_US = 10_000_000;
