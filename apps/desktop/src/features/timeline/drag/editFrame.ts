import { t } from '@lingui/core/macro';
import type { Project } from '../../../bindings/Project';
import { activeIntlLocale } from '../../../i18n';
import { formatSeconds } from '../../../lib/format';
import { snapToFrame } from '../../../lib/time';
import { findItem, sourceLabel } from '../../../lib/timeline';
import { moveRefusal } from '../keys/groupActions';
import { countOf, groupMoveOps, groupReorderOps, membersOf } from '../keys/groupOps';
import { mainIndex } from '../model/index';
import { attachedTo } from '../model/moveOps';
import { editOps, editRoom, heldTo, type EditKind, type EditLimit } from '../model/editRoom';
import { showEditFrames } from '../../../state/editFrames';
import { editPair } from '../media/editFrames';
import { usToPx } from '../scale';
import { rollId, shiftBlock, sizeBlock, slipBlock } from './registry';
import type { Gesture, Session } from './session';

type Edit = Extract<Gesture, { kind: 'edit' }>;
type Group = Extract<Gesture, { kind: 'group' }>;

const NONE: readonly string[] = [];

/** "+0.5 s", "−1.0 s": a change in time, to a tenth. */
function change(us: number): string {
  const tenths = Math.round(us / 100_000);
  const sign = tenths > 0 ? '+' : tenths < 0 ? '−' : '';
  return `${sign}${formatSeconds(activeIntlLocale(), Math.abs(tenths) / 10)}`;
}

/** What holds an edit at the end of its room, said plainly; `name` is the clip whose footage or length ran out. */
function limitNote(held: EditLimit, name: string): string {
  if (held === 'footage')
    return t({ id: 'timeline.edit.noFootage', message: `${name} has no more footage that way` });
  if (held === 'length') return t({ id: 'timeline.edit.shortest', message: `${name} can't get shorter` });
  if (held === 'transition')
    return t({ id: 'timeline.edit.transition', message: 'A transition needs the rest of that footage' });
  return t({ id: 'timeline.edit.locked', message: 'A layer on a locked track would move' });
}

/**
 * The clip a held edit ran out at: a slip's own; at a cut, the clip before it when its footage ran out going later or
 * its length going earlier, else the clip after it.
 */
function heldName(project: Project, kind: EditKind, i: number, by: number, held: EditLimit): string {
  const main = project.timeline.main;
  const before = kind === 'roll' ? i : i - 1;
  const k = kind === 'slip' ? i : (by > 0) === (held === 'footage') ? before : i + 1;
  const clip = main[k];
  return clip ? sourceLabel(project, clip.source) : '';
}

/**
 * One frame of a roll, slip or slide (`g.edit`): its change follows the pointer on the frame grid (a roll's cut, a
 * slide's clip and a slip's footage go with it), held to the room the model allows (`editRoom`), and the tip says the
 * change in time and, at the end of its room, what holds it. The clips are drawn at their new sizes (a slip's filmstrip
 * moves inside its clip), and the preview shows the frame at the edit: the cut after a roll, the slid clip's first
 * frame, the slipped clip's new first frame. False: the clip is gone.
 */
export function frameEdit(s: Session, g: Edit, project: Project, dtUs: number): boolean {
  const main = project.timeline.main;
  if (s.fromMain !== main) {
    s.fromMain = main;
    s.from = main.findIndex((c) => c.id === g.clipId);
    s.room = undefined;
  }
  const i = s.from;
  if (i < 0) return false;
  s.room ??= editRoom(project, g.edit, i);
  const room = s.room;
  const index = mainIndex(main);
  const step = snapToFrame(dtUs, s.fps);
  // A slip's footage follows the pointer: dragged right, earlier footage shows.
  const wanted = g.edit === 'slip' ? -step : step;
  const { by, held } = room ? heldTo(room, wanted) : { by: 0, held: null };
  const px = usToPx(by, s.pps);
  const box = (k: number) => [usToPx(index.starts[k], s.pps), usToPx(index.ends[k] - index.starts[k], s.pps)];
  if (g.edit === 'roll' && i + 1 < main.length) {
    const [l, lw] = box(i);
    const [r, rw] = box(i + 1);
    sizeBlock(main[i].id, l, lw + px);
    sizeBlock(main[i + 1].id, r + px, rw - px);
    shiftBlock(rollId(main[i].id), px);
    s.edgeUs = index.ends[i] + by;
  } else if (g.edit === 'slip') {
    slipBlock(g.clipId, px);
    s.edgeUs = index.starts[i];
  } else if (g.edit === 'slide' && i > 0 && i + 1 < main.length) {
    const [l, lw] = box(i - 1);
    const [r, rw] = box(i + 1);
    sizeBlock(main[i - 1].id, l, lw + px);
    sizeBlock(main[i + 1].id, r + px, rw - px);
    shiftBlock(g.clipId, px);
    for (const layer of attachedTo(project.timeline.tracks).get(g.clipId) ?? NONE) shiftBlock(layer, px);
    shiftBlock(rollId(main[i - 1].id), px);
    shiftBlock(rollId(g.clipId), px);
    s.edgeUs = index.starts[i] + by;
  }
  // The two frames that meet at the edit show beside the preview's (`EditFrames`).
  showEditFrames(editPair(project, g.edit, i, by), g.edit === 'slip');
  s.ops = editOps(g.edit, g.clipId, by);
  s.dryOps = null;
  s.label = g.edit === 'roll' ? 'timeline.roll.undo' : g.edit === 'slip' ? 'timeline.slip.undo' : 'timeline.slide.undo';
  s.labelParams = undefined;
  const amount = change(by);
  const label = g.label;
  const edit =
    g.edit === 'roll'
      ? t({ id: 'timeline.roll.tip', message: `Roll ${amount}`, comment: 'Tip while rolling a cut; {amount} e.g. +0.5 s' })
      : g.edit === 'slip'
        ? t({ id: 'timeline.slip.tip', message: `Slip ${amount}`, comment: "Tip while slipping a clip's footage" })
        : t({ id: 'timeline.slide.tip', message: `Slide ${amount}`, comment: 'Tip while sliding a clip' });
  const note = held === null ? '' : limitNote(held, heldName(project, g.edit, i, wanted, held));
  s.view.showTip(note ? `${edit} · ${note}` : edit, s.last.x);
  s.said =
    g.edit === 'roll'
      ? t({ id: 'timeline.roll.said', message: `Rolled the cut after ${label} by ${amount}` })
      : g.edit === 'slip'
        ? t({ id: 'timeline.slip.said', message: `Slipped ${label} by ${amount}` })
        : t({ id: 'timeline.slide.said', message: `Slid ${label} by ${amount}` });
  return true;
}

/**
 * One frame of a selected group's move: its layers go `dtUs` later on the frame grid, each the same number of track
 * rows as the pressed one (while every track shows), offsets kept. With clips in it, its clips go together to the
 * nearest place on the main track and its other layers as far as they did (`groupReorderOps`). Refused with a
 * sentence when one can't go there (a locked track, a row of another kind, before 0:00); then nothing moves.
 */
export function frameGroup(s: Session, g: Group, project: Project, dtUs: number, now: Set<string>): boolean {
  const m = membersOf(project, g.ids);
  if (countOf(m) === 0) return false;
  const grabbed = findItem(project, g.grabId);
  let rows = 0;
  let dy = 0;
  if (grabbed) {
    const row = s.dropRow !== undefined ? s.dropRow : s.view.rowAt(s.last.y, grabbed.track.kind);
    const tracks = project.timeline.tracks;
    if (row && 'trackId' in row) {
      const to = tracks.findIndex((track) => track.id === row.trackId);
      if (to >= 0) {
        rows = to - tracks.indexOf(grabbed.track);
        dy = row.dy;
      }
    }
  }
  const dt = snapToFrame(dtUs, s.fps);
  // With clips in it, the clips go together to a place on the main track, and its other layers as far as they did.
  const reorder = m.clips.length > 0 ? groupReorderOps(project, m, dt) : null;
  const ops = reorder === null ? groupMoveOps(project, m, dt, rows) : typeof reorder === 'string' ? reorder : reorder.ops;
  s.dryOps = null;
  s.edgeUs = null;
  s.label = 'timeline.group.move';
  if (typeof ops === 'string') {
    s.ops = [];
    s.labelParams = undefined;
    s.view.showTip(moveRefusal(ops), s.last.x);
    return true;
  }
  const moved = reorder !== null && typeof reorder !== 'string' ? reorder.deltaUs : dt;
  const shift = (id: string, us: number, y = 0) => {
    shiftBlock(id, usToPx(us, s.pps), y);
    now.add(id);
  };
  if (reorder !== null && typeof reorder !== 'string') {
    // Every clip that moves, and the layers that follow it.
    const attached = attachedTo(project.timeline.tracks);
    for (const [id, us] of reorder.shifts) {
      shift(id, us);
      for (const layer of attached.get(id) ?? NONE) shift(layer, us);
    }
  }
  for (const { item } of m.layers)
    if (!now.has(item.id)) shift(item.id, moved, reorder === null ? dy : 0);
  s.ops = ops;
  const count = countOf(m);
  s.labelParams = { count };
  s.said = t({ id: 'timeline.group.moved', message: `Moved ${count} items` });
  s.view.showTip(change(moved), s.last.x);
  return true;
}
