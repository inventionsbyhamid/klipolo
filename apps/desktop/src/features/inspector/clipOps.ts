import type { Clip } from '../../bindings/Clip';
import type { ClipPatch } from '../../bindings/ClipPatch';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Framing } from '../../bindings/Framing';
import type { Project } from '../../bindings/Project';
import { soundSources } from '../../lib/recording';
import { assetById } from '../../lib/timeline';

/**
 * `clip`'s framing in `format` set to `framing`; the other formats keep theirs: `clip` as the project has it when the
 * change is built (`editClip`, a slider's ops).
 */
export function framingPatch(clip: Clip, format: FormatKind, framing: Framing): ClipPatch {
  return { framing: { ...clip.framing, [format]: framing } };
}

/** Whether `clip` has any sound for its volume to change: its file's, or a recording's sound sources. */
export function clipHasSound(project: Pick<Project, 'assets' | 'recordings'>, clip: Clip): boolean {
  const source = clip.source;
  if (source.type === 'asset') return !!assetById(project, source.id)?.hasAudio;
  const recording = project.recordings.find((r) => r.id === source.id);
  return !!recording && soundSources(project, recording).length > 0;
}
