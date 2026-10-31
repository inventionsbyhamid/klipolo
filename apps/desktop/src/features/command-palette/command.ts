import type { ContribId } from '../../bindings/ContribId';
import type { ParamValue } from '../../bindings/ParamValue';
import type { DropTarget } from '../timeline/drops';

/*
 * What a command is: the keys, the player bar, the timeline's menus and the rows run it, and each names the edit it
 * makes (an op's name, or what it does). `useCommands` gathers them.
 */

/**
 * How a command was run, when it matters. A track's header or menu, a media row, an effect row or a block's menu set
 * what they act on, and a Finder drop the files; chat's edits compile in Rust and reach none of these.
 */
export interface RunOptions {
  /** ⇧ was held: the bigger step (ten frames for a nudge). */
  large?: boolean;
  /** A held key's repeat: a command that can't act doesn't say why again. */
  repeat?: boolean;
  /** Run from this block's menu: it acts on that block alone (Split at playhead on a clip). */
  block?: string;
  /** Run from this track's header or menu: a track command acts on that track (`useTrackCommands`). */
  track?: string;
  /** A rename's new name (a track header's field); without one, Rename track makes the name a field in its header. */
  name?: string;
  /** The state Hide track, Mute track and Lock track end in (a header's toggle); without one they switch. */
  hide?: boolean;
  mute?: boolean;
  lock?: boolean;
  /** The imported file a media command puts in the video (its asset id: a media row, its menu or the palette on it). */
  asset?: string;
  /** Where media goes (`media-add`, `import`): a cut of the main track, or a time on a layer's lane. */
  target?: DropTarget;
  /** The files `import` brings in (a Finder drop) instead of asking with the file picker. */
  paths?: string[];
  /** The effect `effect-add` puts on the block (a contribution id: `klipolo.builtin/effect.blur`). */
  effect?: ContribId;
  /** Which effect of the block's stack an effect command acts on (its id in the stack: an effect row). */
  effectId?: string;
  /** The state `effect-toggle` ends in (an effect's switch); without one it switches to the other state. */
  enabled?: boolean;
  /** Settings `effect-set` sets (and `effect-add` starts with), by key, and the keys it takes back to their defaults. */
  params?: Record<string, ParamValue>;
  reset?: string[];
  /** The state Solo track ends in (a header's toggle); without one it switches to the other state. */
  solo?: boolean;
}

export interface Command {
  id: string;
  /** What it does: the op it makes (`moveItem`), or a name for what it does (`playback`). */
  action: string;
  label: string;
  group: 'edit' | 'playback' | 'view' | 'project' | 'looks';
  /** The keys: one combination ("⌘Z"), or several keys shown one by one (J, K, L). */
  shortcut?: string | readonly string[];
  /** Other words people search with, e.g. "cut" for Split clip (in the language shown). */
  keywords?: string[];
  enabled: boolean;
  /** Why it is off, said with it where it is listed (a block's menu item's tooltip): "Mic is on its own track here." */
  reason?: string;
  /** Listed in the palette but inert: the feature ships in the next build. */
  nextBuild?: boolean;
  /**
   * Left out of the palette, which has another entry that does the same (K's Pause: the palette's Play/Pause shows K)
   * or can't give what it needs (an effect's settings, where media goes). Keys, rows and menus run it.
   */
  paletteHidden?: boolean;
  /** Listed only while a media row or an effect row has focus: the palette can't name one by itself. */
  needs?: 'asset' | 'effect';
  /**
   * With `needs`: whether it can act on the row that has focus, as that row's own menu decides (a sound has no Add to
   * main track; the first effect has no Move up). Without it, any such row will do.
   */
  fits?: (row: FocusedRow) => boolean;
  run: (options?: RunOptions) => void;
}

/** Search words from a translated, comma-separated list ("cut, trim"). */
export function searchWords(list: string): string[] {
  return list
    .split(/[,،、，]/)
    .map((word) => word.trim())
    .filter(Boolean);
}

/** The media row or effect row that had focus when the palette opened: what a `needs` entry acts on. */
export interface FocusedRow {
  /** A media row's file (`data-asset-id`). */
  asset?: string;
  /**
   * An effect row's effect, by its id in the stack (`data-effect-id`), and that stack's block (`data-effect-block`).
   */
  effectId?: string;
  block?: string;
}
