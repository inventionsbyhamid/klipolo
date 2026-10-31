import type { Clip } from '../../bindings/Clip';
import type { ClipPatch } from '../../bindings/ClipPatch';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Framing } from '../../bindings/Framing';

/** The speeds the clip card offers; the clip's length follows (the model rescales it). */
export const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2];

/** The speeds to offer a clip playing at `current`: the usual ones, and `current` when it is another (set elsewhere). */
export function speedChoices(current: number): number[] {
  return SPEEDS.includes(current) ? SPEEDS : [...SPEEDS, current].sort((a, b) => a - b);
}

/**
 * `clip`'s framing in `format` set to `framing`; the other formats keep theirs: `clip` as the project has it when the
 * change is built (`editClip`, a slider's ops).
 */
export function framingPatch(clip: Clip, format: FormatKind, framing: Framing): ClipPatch {
  return { framing: { ...clip.framing, [format]: framing } };
}
