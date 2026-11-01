import type { Clip } from '../../../bindings/Clip';
import type { Project } from '../../../bindings/Project';
import { frameUs } from '../../../lib/time';
import type { FileFrame } from '../../../state/editFrames';
import type { EditKind } from '../model/editRoom';
import { clipFiles } from './blockFiles';
import { fileUs } from './runs';

/** The frame clip `clip` shows at its source time `sourceUs`: from the run of its footage playing then. */
export function frameOf(project: Project, clip: Clip, sourceUs: number): FileFrame | null {
  const files = clipFiles(project, clip.source);
  const film = files?.runs?.film;
  if (film && film.length > 0) {
    const run =
      film.find((r) => r.fromUs <= sourceUs && sourceUs < r.toUs) ??
      film.reduce((a, b) => (Math.abs(b.fromUs - sourceUs) < Math.abs(a.fromUs - sourceUs) ? b : a));
    return { path: run.path, fileUs: Math.max(0, fileUs(run, sourceUs)) };
  }
  return files?.thumbPath ? { path: files.thumbPath, fileUs: Math.max(0, sourceUs) } : null;
}

/**
 * The two frames that meet at an edit of `kind` by `byUs` at main-track clip `i` (a roll's at the cut after it), as
 * the clips will play them: the outgoing clip's new last frame and the incoming one's new first; a slip's own new first
 * and last.
 */
export function editPair(
  project: Project,
  kind: EditKind,
  i: number,
  byUs: number,
): readonly [FileFrame | null, FileFrame | null] | null {
  const main = project.timeline.main;
  const frame = frameUs(project.settings.fps);
  const shift = (clip: Clip) => Math.round(byUs * clip.speed);
  const last = (clip: Clip, out: number) => frameOf(project, clip, out - Math.round(frame * clip.speed));
  if (kind === 'slip') {
    const clip = main[i];
    return clip
      ? [frameOf(project, clip, clip.sourceInUs + shift(clip)), last(clip, clip.sourceOutUs + shift(clip))]
      : null;
  }
  // A roll's two clips; a slide's neighbours, the one before ending where the slid clip now starts and the one after
  // starting where it now ends.
  const [before, after] = kind === 'roll' ? [main[i], main[i + 1]] : [main[i - 1], main[i + 1]];
  if (!before || !after) return null;
  return [
    last(before, before.sourceOutUs + shift(before)),
    frameOf(project, after, after.sourceInUs + shift(after)),
  ];
}
