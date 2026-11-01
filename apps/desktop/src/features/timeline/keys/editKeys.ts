import { t } from '@lingui/core/macro';
import type { Project } from '../../../bindings/Project';
import { activeIntlLocale } from '../../../i18n';
import type { LabelId } from '../../../lib/labels';
import { formatSeconds } from '../../../lib/format';
import { frameStartUs } from '../../../lib/time';
import { sourceLabel } from '../../../lib/timeline';
import { keyBurst } from '../../../state/keyBurst';
import { usePlayback } from '../../../state/playback';
import { toast } from '../../../state/toasts';
import { announce } from '../announce';
import { dragging } from '../drag/controller';
import { editOps, editRoom, heldTo, type EditLimit } from '../model/editRoom';
import { clipIndexAt, mainIndex } from '../model/index';
import { editable } from './timelineActions';

/** The cut nearest the playhead: the index of the clip before it, or −1 when there is none. */
export function cutNearPlayhead(project: Project, playheadUs: number): number {
  const index = mainIndex(project.timeline.main);
  let best = -1;
  for (let i = 0; i + 1 < project.timeline.main.length; i++)
    if (best < 0 || Math.abs(index.ends[i] - playheadUs) < Math.abs(index.ends[best] - playheadUs)) best = i;
  return best;
}

/** The clip under the playhead, or −1. */
export function clipUnderPlayhead(project: Project, playheadUs: number): number {
  return clipIndexAt(mainIndex(project.timeline.main), playheadUs);
}

/** Why a roll or slip by key went no further, said once per held key. */
function heldSentence(held: EditLimit): string {
  if (held === 'footage') return t({ id: 'timeline.edit.noMoreFootage', message: 'There is no more footage that way.' });
  if (held === 'length') return t({ id: 'timeline.edit.tooShort', message: "A clip can't get any shorter." });
  if (held === 'transition')
    return t({ id: 'timeline.edit.transitionHolds', message: 'A transition needs the rest of that footage.' });
  return t({ id: 'timeline.edit.lockedHolds', message: 'A layer on a locked track would move.' });
}

/** "+0.5 s", "−0.1 s". */
function change(us: number): string {
  const tenths = Math.round(us / 100_000);
  const sign = tenths > 0 ? '+' : tenths < 0 ? '−' : '';
  return `${sign}${formatSeconds(activeIntlLocale(), Math.abs(tenths) / 10)}`;
}

/**
 * One press of a roll (⌥, and ⌥.) or a slip (⌥⇧, and ⌥⇧.) key, `direction` a frame: the cut nearest the playhead
 * rolls, or the clip under it slips (later footage for ⌥⇧.). A held key's presses add up into one undo step
 * (`keyBurst`), held to the edit's room, and say once why they stop.
 */
function press(kind: 'roll' | 'slip', direction: 1 | -1, quiet: boolean) {
  const project = editable();
  if (!project) return;
  const at = usePlayback.getState().playheadUs;
  const i = kind === 'roll' ? cutNearPlayhead(project, at) : clipUnderPlayhead(project, at);
  if (i < 0) {
    if (!quiet)
      toast.info(
        kind === 'roll'
          ? t({ id: 'timeline.roll.noCut', message: 'There is no cut to roll.' })
          : t({ id: 'timeline.slip.noClip', message: "The playhead isn't over a clip." }),
      );
    return;
  }
  const clip = project.timeline.main[i];
  const key = `${kind}:${clip.id}`;
  const before = keyBurst.pending(key)?.total ?? 0;
  const frames = before + direction;
  const room = editRoom(project, kind, i);
  const fps = project.settings.fps;
  // `n` frames of timeline, either way, on the frame grid.
  const framesUs = (n: number) => Math.sign(n) * frameStartUs(Math.abs(n), fps) || 0;
  const was = room ? heldTo(room, framesUs(before)).by : 0;
  const { by, held } = room ? heldTo(room, framesUs(frames)) : { by: 0, held: 'footage' as const };
  // Held where the presses before it already were: nothing more to add.
  if (by === was) {
    if (!quiet && held !== null) toast.info(heldSentence(held));
    return;
  }
  const label = sourceLabel(project, clip.source);
  const amount = change(by);
  const said =
    kind === 'roll'
      ? t({ id: 'timeline.roll.said', message: `Rolled the cut after ${label} by ${amount}` })
      : t({ id: 'timeline.slip.said', message: `Slipped ${label} by ${amount}` });
  const undo: LabelId = kind === 'roll' ? 'timeline.roll.undo' : 'timeline.slip.undo';
  keyBurst.push(undo, editOps(kind, clip.id, by), key, {
    total: frames,
    done: (ok) => {
      if (ok && by !== 0) announce(said);
    },
  });
}

/** The roll and slip keys: a press for the edit at the playhead, adding to a held key's burst. `quiet`: a repeat. */
export function editByKey(kind: 'roll' | 'slip', direction: 1 | -1, quiet = false) {
  // A drag owns the timeline until it lets go.
  if (dragging()) return;
  const project = editable();
  if (!project) return;
  const at = usePlayback.getState().playheadUs;
  const i = kind === 'roll' ? cutNearPlayhead(project, at) : clipUnderPlayhead(project, at);
  const key = i < 0 ? `${kind}:` : `${kind}:${project.timeline.main[i].id}`;
  if (keyBurst.pending(key)) press(kind, direction, quiet);
  else keyBurst.after(() => press(kind, direction, quiet), key);
}
