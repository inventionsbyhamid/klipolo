import type { Clip } from '../../bindings/Clip';
import type { ClipPatch } from '../../bindings/ClipPatch';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Framing } from '../../bindings/Framing';
import { clipDurationUs } from '../../lib/timeline';

/**
 * `clip`'s framing in `format` set to `framing`; the other formats keep theirs: `clip` as the project has it when the
 * change is built (`editClip`, a slider's ops).
 */
export function framingPatch(clip: Clip, format: FormatKind, framing: Framing): ClipPatch {
  return { framing: { ...clip.framing, [format]: framing } };
}

/** Fades from and to black, in microseconds: None, 0.5 s and 1 s. */
const CLIP_FADES = [0, 500_000, 1_000_000];

/** One end of a clip's fades, as its card offers it. */
export interface FadeEnd {
  /** The fade it has there (µs). */
  us: number;
  /**
   * The lengths offered: None, 0.5 s and 1 s, and the fade it has when that is another length (set elsewhere), so the
   * control shows what is set. `tooLong`: with the other end's fade it would be longer than the clip (the model
   * refuses that).
   */
  choices: { us: number; tooLong: boolean }[];
  /** A transition blends at this end's cut: it replaces the fade there (the model drops it), so none can be set. */
  blends: boolean;
}

/** Clip `clip`'s fade in and fade out as its card offers them, by its place in `main`. */
export function clipFades(main: Clip[], clip: Clip): { in: FadeEnd; out: FadeEnd } {
  const durationUs = clipDurationUs(clip);
  const at = main.findIndex((c) => c.id === clip.id);
  const end = (us: number, otherUs: number, blends: boolean): FadeEnd => ({
    us,
    blends,
    choices: [...new Set([...CLIP_FADES, us])]
      .sort((a, b) => a - b)
      .map((choice) => ({ us: choice, tooLong: choice + otherUs > durationUs })),
  });
  return {
    // The clip before has a transition into this one, or this one into the next.
    in: end(clip.fadeInUs, clip.fadeOutUs, at > 0 && !!main[at - 1].transitionOut),
    out: end(clip.fadeOutUs, clip.fadeInUs, at >= 0 && at + 1 < main.length && !!clip.transitionOut),
  };
}
