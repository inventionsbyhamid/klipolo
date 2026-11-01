import { t } from '@lingui/core/macro';
import type { Edge } from '../../../bindings/Edge';
import type { Project } from '../../../bindings/Project';
import { activeIntlLocale } from '../../../i18n';
import { formatSeconds } from '../../../lib/format';
import { formatPrecise, frameIndexAt, frameStartUs, snapToFrame } from '../../../lib/time';
import { clipDurationUs, findItem, itemLabel, sourceLabel } from '../../../lib/timeline';
import { useUi } from '../../../state/ui';
import { LAYER_MIN_PX } from '../layout';
import { mainIndex } from '../model/index';
import { attachedTo } from '../model/moveOps';
import { refitNote, trimRefit } from '../model/refit';
import { snapEdges, snapTargets } from '../model/snap';
import { trimClip, trimItemEdge } from '../model/trim';
import { visibleRange } from '../model/visible';
import { pxToUs, usToPx } from '../scale';
import { SNAP_PX, type Gesture, type Session } from './session';
import { shiftBlock, sizeBlock } from './registry';

type Trim = Extract<Gesture, { kind: 'trim' }>;

const NONE: readonly string[] = [];

/** The start of the last frame before `us`: an edge's last frame, as playback shows it. */
const frameBefore = (us: number, fps: number) => frameStartUs(frameIndexAt(us - 1, fps), fps);

/** The trim of `id`'s `edge` (a layer, or a clip on the main track) as the project is now; null when it's neither. */
export function trimGesture(project: Project, id: string, edge: Edge): Trim | null {
  const found = findItem(project, id);
  if (found) {
    const label = itemLabel(project, found.item);
    return { kind: 'trim', target: 'item', itemId: id, trackId: found.track.id, edge, label };
  }
  const clip = project.timeline.main.find((c) => c.id === id);
  return clip
    ? { kind: 'trim', target: 'clip', clipId: id, edge, label: sourceLabel(project, clip.source) }
    : null;
}

/**
 * `us` on the frame grid, held to a snap target within 8 px unless snapping is off or ⌘ is held. The targets
 * are built once per gesture; `guide` is the target it held to.
 */
function snapped(s: Session, us: number, targets: () => Float64Array): { us: number; guide: number | null } {
  const at = snapToFrame(us, s.fps);
  if (!useUi.getState().snapping || s.last.meta) return { us: at, guide: null };
  s.targets ??= targets();
  const hit = snapEdges([at], s.targets, pxToUs(SNAP_PX, s.pps));
  return hit ? { us: at + hit.deltaUs, guide: hit.atUs } : { us: at, guide: null };
}

/**
 * The tip ("0:09.0, −1.0 s": the new length and the change) and what is announced on release; `note` says what the
 * trim does to transitions ("A transition is shortened.").
 */
function tell(s: Session, g: Trim, lengthUs: number, changeUs: number, note = '') {
  const tenths = Math.round(changeUs / 100_000);
  const sign = tenths > 0 ? '+' : tenths < 0 ? '−' : '';
  const length = formatPrecise(lengthUs);
  const change = `${sign}${formatSeconds(activeIntlLocale(), Math.abs(tenths) / 10)}`;
  const label = g.label;
  s.view.showTip(
    note
      ? t({
          id: 'timeline.trim.tipNote',
          message: `${length}, ${change} · ${note}`,
          comment:
            'Tip while trimming: the new length, the change and a sentence on what it does to transitions',
        })
      : t({
          id: 'timeline.trim.tip',
          message: `${length}, ${change}`,
          comment: 'Tip while trimming: the new length (e.g. 0:09.0) and the change (e.g. −1.0 s)',
        }),
    s.last.x,
  );
  s.said = note
    ? t({
        id: 'timeline.trim.saidNote',
        message: `Trimmed ${label} to ${length}. ${note}`,
        comment:
          'Screen-reader note; {note} is a sentence on what it does to transitions, e.g. “A transition is removed.”',
      })
    : t({ id: 'timeline.trim.said', message: `Trimmed ${label} to ${length}` });
}

/**
 * One frame of a trim: the pressed edge goes where the pointer is (`dtUs` from where it was pressed), on the frame
 * grid, snapping like a move, and held to the media and (a layer) its neighbours on the track by the pure builders
 * chat's tools use too. The block is drawn at its new size; a main clip's end trim slides the clips after it and
 * their attached layers (ids in `now`), as the model's ripple will. Sets `s.ops`, `s.label`, `s.said`, the edge
 * frame the preview shows (`s.edgeUs`) and the dry run that shows it (`s.dryOps`). False: the block is gone.
 *
 * An edge (a clip's or a layer's) never snaps back to where it began, as a moved layer never snaps to its own edges:
 * a trim of a few pixels stays a trim.
 */
export function frameTrim(s: Session, g: Trim, project: Project, dtUs: number, now: Set<string>): boolean {
  return g.target === 'clip' ? trimClipFrame(s, g, project, dtUs, now) : trimItemFrame(s, g, project, dtUs);
}

function trimClipFrame(
  s: Session,
  g: Extract<Trim, { target: 'clip' }>,
  project: Project,
  dtUs: number,
  now: Set<string>,
): boolean {
  const main = project.timeline.main;
  if (s.fromMain !== main) {
    s.fromMain = main;
    s.from = main.findIndex((c) => c.id === g.clipId);
  }
  const i = s.from;
  if (i < 0) return false;
  const index = mainIndex(main);
  const [startUs, endUs] = [index.starts[i], index.ends[i]];
  const from = g.edge === 'start' ? startUs : endUs;
  // An end trim ripples: what comes after the clip moves with the edge, so it isn't a target.
  const edge = snapped(s, from + dtUs, () =>
    snapTargets(project, s.playheadUs, new Set(), g.edge === 'end' ? i : undefined).filter((t) => t !== from),
  );
  const { sourceUs, deltaUs } = trimClip(project, main[i], g.edge, edge.us - from, s.fps);
  const left = usToPx(startUs, s.pps);
  const width = usToPx(endUs - startUs, s.pps);
  const px = usToPx(deltaUs, s.pps);
  if (g.edge === 'end') {
    sizeBlock(g.clipId, left, width + px);
    // The ripple, live: the later clips slide with the end, with their layers. Only the mounted ones can (the lanes
    // mount the view and up to 1.5 views each side), so the work doesn't grow with the trim or the project.
    const visUs = pxToUs(s.view.scroller.scrollLeft, s.pps);
    const [first, end] = visibleRange(index, visUs - 2 * s.viewUs, visUs + 3 * s.viewUs);
    const attached = attachedTo(project.timeline.tracks);
    const slide = (id: string) => {
      shiftBlock(id, px);
      now.add(id);
    };
    for (let j = Math.max(first, i + 1); j < end; j++) {
      slide(main[j].id);
      for (const layer of attached.get(main[j].id) ?? NONE) slide(layer);
    }
  } else sizeBlock(g.clipId, left + px, width - px);
  s.view.showGuide(edge.guide !== null && from + deltaUs === edge.guide ? edge.guide : null);
  s.ops = deltaUs ? [{ type: 'trimClip', id: g.clipId, edge: g.edge, sourceUs }] : [];
  s.label = 'timeline.trim.clip.undo';
  const change = g.edge === 'end' ? deltaUs : -deltaUs;
  // The model refits the transitions into and out of the clip: the trim says when it loses or shortens one.
  const field = g.edge === 'start' ? 'sourceInUs' : 'sourceOutUs';
  const trimmed = { ...main[i], [field]: sourceUs };
  const note = deltaUs ? refitNote(trimRefit(project, i, trimmed)) : '';
  // The preview shows the edge in the dry run: the clip's new first frame (the track closes up, so it still starts
  // where it did), or its new last one. It is the clip's own frame, never a transition's blend: the dry run leaves out
  // the transition at that edge, one decode instead of two (the commit keeps it).
  s.edgeUs = g.edge === 'start' ? startUs : frameBefore(startUs + clipDurationUs(trimmed), s.fps);
  const before = g.edge === 'start' ? main[i - 1] : main[i];
  s.dryOps = before?.transitionOut
    ? [...s.ops, { type: 'setTransition', clipId: before.id, transition: null }]
    : null;
  tell(s, g, endUs - startUs + change, change, note);
  return true;
}

function trimItemFrame(
  s: Session,
  g: Extract<Trim, { target: 'item' }>,
  project: Project,
  dtUs: number,
): boolean {
  const found = findItem(project, g.itemId);
  if (!found) return false;
  const { item, track } = found;
  const endUs = item.startUs + item.durationUs;
  const from = g.edge === 'start' ? item.startUs : endUs;
  // Like a clip's edge, it never snaps back to where it began: a trim of a few pixels stays a trim.
  const edge = snapped(s, from + dtUs, () =>
    snapTargets(project, s.playheadUs, new Set([item.id])).filter((t) => t !== from),
  );
  const atUs = trimItemEdge(project, track, item, g.edge, edge.us, s.fps);
  const [start, end] = g.edge === 'start' ? [atUs, endUs] : [item.startUs, atUs];
  // As `ItemBlock` draws it: never narrower than its edge and padding.
  sizeBlock(g.itemId, usToPx(start, s.pps), Math.max(usToPx(end - start, s.pps), LAYER_MIN_PX));
  s.view.showGuide(edge.guide !== null && atUs === edge.guide ? edge.guide : null);
  s.ops = atUs === from ? [] : [{ type: 'trimItem', id: g.itemId, edge: g.edge, atUs }];
  s.dryOps = null;
  s.label = 'timeline.trim.layer.undo';
  // The preview shows the edge: the layer's new first frame, or its new last one.
  s.edgeUs = g.edge === 'start' ? atUs : frameBefore(atUs, s.fps);
  tell(s, g, end - start, end - start - item.durationUs);
  return true;
}
