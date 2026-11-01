import type { Clip } from '../../../bindings/Clip';
import type { Edge } from '../../../bindings/Edge';
import type { Op } from '../../../bindings/Op';
import type { Snapshot } from '../../../bindings/Snapshot';
import type { TrackKind } from '../../../bindings/TrackKind';
import { create } from 'zustand';
import { newId } from '../../../lib/defaults';
import { commitInFlight } from '../../../state/commitQueue';
import { useDragState } from '../../../state/dragState';
import { showEditFrames } from '../../../state/editFrames';
import { changedSince, type Built } from '../../../state/editBase';
import { usePlayback } from '../../../state/playback';
import { captureBase, useProject } from '../../../state/project';
import { useUi } from '../../../state/ui';
import { gutterPx } from '../layout';
import { mainIndex } from '../model/index';
import { pxToUs } from '../scale';
import { blockElement, clearShifts, rollId } from './registry';
import type { LabelId, LabelParams } from '../../../lib/labels';
import type { EditKind, EditRoom } from '../model/editRoom';

/** A press that travels less than this (CSS px) is a click, not a drag. */
export const DRAG_SLOP_PX = 3;
/** A dragged edge this near (screen px) a snap target holds to it. */
export const SNAP_PX = 8;

export type Gesture =
  | { kind: 'clip'; clipId: string; label: string; grabUs: number }
  | { kind: 'item'; itemId: string; label: string; trackId: string; trackKind: TrackKind }
  | { kind: 'trim'; target: 'clip'; clipId: string; edge: Edge; label: string }
  | { kind: 'trim'; target: 'item'; itemId: string; trackId: string; edge: Edge; label: string }
  /** A roll of the cut after clip `clipId`, or a slip or slide of that clip (`drag/editFrame`). */
  | { kind: 'edit'; edit: EditKind; clipId: string; label: string }
  /** The selected group's layers, moved together by a press on `grabId`, one of them. */
  | { kind: 'group'; grabId: string; ids: readonly string[]; label: string };

export type RowHit = { trackId: string; dy: number } | { newTrack: { index: number }; dy: number };

export interface DragView {
  scroller: HTMLElement;
  /** Expanded mode: the track row of `kind` under `clientY` (or the new-track zone); compact: null. */
  rowAt(clientY: number, kind: TrackKind): RowHit | null;
  showGuide(us: number | null): void;
  showTip(text: string | null, clientX?: number): void;
}

/** What a gesture reads of a pointer event (React's or the DOM's). */
export type PointerLike = Pick<PointerEvent, 'pointerId' | 'clientX' | 'clientY'> & { metaKey?: boolean };

/** One press of a block or an edge, from the press to its commit (`drag/controller`). */
export interface Session {
  gesture: Gesture;
  view: DragView;
  pointerId: number;
  origin: { x: number; y: number; scroll: number };
  last: { x: number; y: number; meta: boolean };
  /** Where the lanes were scrolled when it was dropped, once it is (a queued drop draws there, not where they are). */
  scrollAt: number | null;
  /** A queued layer drop: the track row it was dropped on, found at the release (null: none, or not queued). */
  dropRow?: RowHit | null;
  revision: number;
  pps: number;
  /** The view's length (µs) at the press. */
  viewUs: number;
  fps: number;
  expanded: boolean;
  /** The playhead before the press moved it: what the gesture snaps to (a picked layer's own start isn't a magnet). */
  playheadUs: number;
  started: boolean;
  /** The ops last sent as a dry run (null: none is out), so a block held still doesn't resend them every frame. */
  sent: Op[] | null;
  targets: Float64Array | null;
  shifted: Set<string>;
  ops: Op[];
  /** The undo label of `ops` (null until a frame has made some), and its values ("Move 3 items"). */
  label: LabelId | null;
  labelParams?: LabelParams;
  /** A roll's, slip's or slide's room, worked out once per main track (`editRoom`; null: it can't act there). */
  room?: EditRoom | null;
  /**
   * The dry run when it isn't `ops` alone (null: `ops`): a clip's trim beside a transition leaves out the transition at
   * that edge, so the preview shows the clip's own frame there, never the blend. Its commit keeps the transition.
   */
  dryOps: Op[] | null;
  /** A trim's edge frame, in its dry run: what the preview shows while it trims (null: a move). */
  edgeUs: number | null;
  /** The edge frame the preview was last asked to show (null: it shows the playhead's). */
  shownUs: number | null;
  said: string;
  newTrackId: string;
  /** Where a clip was grabbed, from its start: a commit that lands before it lifts moves its start. */
  grabOffsetUs: number;
  /**
   * The copy it is built on: taken at the press, and again if a change lands before it lifts (it has drawn
   * nothing yet). Its commit goes with it, so this window's own commits landing mid-drag let it in, and a change from
   * elsewhere refuses it.
   */
  built: Built;
  /** A change from elsewhere (chat's) merged mid-drag: it draws no more, and its release is refused. */
  stale: boolean;
  /** The dragged or trimmed clip's index in `fromMain` (looked up once per main track). */
  fromMain: Clip[] | null;
  from: number;
  /** What a dragged clip's drop at `to` does to transitions ("removes a transition"), worked out once per target. */
  refit: { main: Clip[]; to: number; text: string } | null;
  stopScroll: (() => void) | null;
}

/** A press of `gesture` on `snapshot`; `from` is a clip's index on the main track (−1 for a layer). */
export function newSession(
  e: PointerLike,
  gesture: Gesture,
  view: DragView,
  snapshot: Snapshot,
  from: number,
  playheadUs: number,
): Session {
  const { pxPerSecond: pps, timelineExpanded: expanded } = useUi.getState();
  const { main } = snapshot.project.timeline;
  return {
    gesture,
    view,
    pointerId: e.pointerId,
    origin: { x: e.clientX, y: e.clientY, scroll: view.scroller.scrollLeft },
    last: { x: e.clientX, y: e.clientY, meta: false },
    scrollAt: null,
    revision: snapshot.history.revision,
    pps,
    // Before the first layout (jsdom) a 1200 px view, as `useViewport` assumes.
    viewUs: pxToUs((view.scroller.clientWidth || 1200) - gutterPx(), pps),
    fps: snapshot.project.settings.fps,
    expanded,
    playheadUs,
    started: false,
    sent: null,
    targets: null,
    shifted: new Set(),
    ops: [],
    label: null,
    dryOps: null,
    edgeUs: null,
    shownUs: null,
    said: '',
    newTrackId: newId(),
    grabOffsetUs: gesture.kind === 'clip' ? gesture.grabUs - mainIndex(main).starts[from] : 0,
    built: captureBase(),
    stale: false,
    fromMain: main,
    from,
    refit: null,
    stopScroll: null,
  };
}

/**
 * Watches what gesture `s` depends on while `live()`. The project changing under it (an undo, say) means it can't be
 * committed and it is cancelled; but a press not yet lifted has drawn nothing (it starts again from
 * the new project, its base too), and a commit landing (a drop's, a nudge the press let go in) is the queue's own: then
 * it carries on from the new project. A change from elsewhere (chat's, merged mid-drag) makes it `outdated`: it stops,
 * and its release is refused, never added to where that change put things. A zoom or the other track view redraws
 * every block under it (cancelled); snapping on or off (N) redraws the drag. Playback starting ends a trim's edge
 * display: playback draws from the playhead, and a paused trim shows its edge again. Either redraws the drag, so its
 * dry run plays with the transitions it keeps and shows the edge without them (`dryOps`).
 */
export function watchSession(
  s: Session,
  live: () => boolean,
  cancel: () => void,
  redraw: () => void,
  outdated: () => void,
): () => void {
  const offProject = useProject.subscribe(({ snapshot: next }) => {
    if (!live() || s.stale || next?.history.revision === s.revision) return;
    if (!s.started && next) {
      s.revision = next.history.revision;
      s.targets = null;
      s.built = captureBase();
    } else if (changedSince(s.built)) outdated();
    else if (commitInFlight() && next) {
      s.revision = next.history.revision;
      s.targets = null;
    } else cancel();
  });
  const offUi = useUi.subscribe((u, prev) => {
    if (!live()) return;
    if (u.pxPerSecond !== s.pps || u.timelineExpanded !== s.expanded) cancel();
    else if (u.snapping !== prev.snapping) redraw();
  });
  const offPlayback = usePlayback.subscribe((p, prev) => {
    if (!live() || p.playing === prev.playing) return;
    if (p.playing) s.shownUs = null;
    redraw();
  });
  return () => {
    offProject();
    offUi();
    offPlayback();
  };
}

/** The blocks a running drag moves: the lanes keep them mounted outside the render window and out of bands. */
export const useDragged = create<{ ids: readonly string[] }>(() => ({ ids: [] }));
export const NONE: readonly string[] = [];

/** What a lift shows: the blocks it moves stay mounted, and a dragged layer's kind opens its new-track zone. */
export function setLifted(ids: readonly string[], itemKind: TrackKind | null) {
  useDragged.setState({ ids });
  useDragState.setState({ itemKind });
}

/** The block a gesture lifts and draws dragged: a roll's is its handle, which leaves the two clips as they are. */
export const draggedId = (g: Gesture) =>
  g.kind === 'group'
    ? g.grabId
    : g.kind === 'edit' && g.edit === 'roll'
      ? rollId(g.clipId)
      : 'clipId' in g
        ? g.clipId
        : g.itemId;

/** Puts down what `s` lifted; its shifts stay while they are being committed (until the new revision draws them). */
export function lower(s: Session, keepShifts: boolean) {
  showEditFrames(null);
  s.stopScroll?.();
  s.stopScroll = null;
  s.view.showGuide(null);
  s.view.showTip(null);
  const el = blockElement(draggedId(s.gesture));
  if (el) delete el.dataset.dragging;
  if (keepShifts) return;
  clearShifts();
  setLifted(NONE, null);
}
