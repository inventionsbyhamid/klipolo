import type { Clip } from '../../bindings/Clip';
import type { ClipPatch } from '../../bindings/ClipPatch';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Framing } from '../../bindings/Framing';

/**
 * `clip`'s framing in `format` set to `framing`; the other formats keep theirs: `clip` as the project has it when the
 * change is built (`editClip`, a slider's ops).
 */
export function framingPatch(clip: Clip, format: FormatKind, framing: Framing): ClipPatch {
  return { framing: { ...clip.framing, [format]: framing } };
}
