import { t } from '@lingui/core/macro';
import { findItem } from '../../../lib/timeline';
import {
  afterCommits,
  beginCommit,
  commitInFlight,
  onAbandon,
  onCloseStart,
} from '../../../state/commitQueue';
import { holdDock, releaseDock } from '../../../state/dockHold';
import { keyBurst } from '../../../state/keyBurst';
import { showFrameAt, usePlayback } from '../../../state/playback';
import { useProject } from '../../../state/project';
import { toast } from '../../../state/toasts';
import { clearPreview, dropPendingPreview, previewOps } from '../../../state/livePreview';
import { announce } from '../announce';
import { gutterPx } from '../layout';
import { attachedTo, sameOps } from '../model/moveOps';
import { pxToUs } from '../scale';
import { startAutoscroll } from './autoscroll';
import { frameClip, frameItem } from './moveFrame';
import { blockElement, clearShifts, shiftBlock } from './registry';
import {
  DRAG_SLOP_PX,
  draggedId,
  lower,
  NONE,
  newSession,
  setLifted,
  watchSession,
  type DragView,
  type Gesture,
  type PointerLike,
  type Session,
} from './session';
import { frameTrim } from './trimFrame';

export { DRAG_SLOP_PX, SNAP_PX, useDragged } from './session';
export type { DragView, Gesture, PointerLike, RowHit, Session } from './session';

/** The live gesture: pressed, and lifted once past the slop. */
let session: Session | null = null;
let stopWatching: (() => void) | null = null;
let scheduled = false;
/**
 * While a commit is in flight (a drag's, a nudge's, a keyboard edit's: `commitQueue`), nothing is drawn or built
 * against the project it is about to replace: a press lifts once it lands, and a drop made meanwhile waits in `queued`,
 * then builds its ops from the project that commit left (or the same one, if it failed).
 */
const queued: Session[] = [];
let waiting = false;

const nextFrame = (cb: () => void) =>
  typeof requestAnimationFrame === 'function' ? requestAnimationFrame(cb) : setTimeout(cb, 16);
const pastSlop = (s: Session) => Math.hypot(s.last.x - s.origin.x, s.last.y - s.origin.y) >= DRAG_SLOP_PX;

export function dragging(): boolean {
  return session?.started ?? false;
}

/**
 * True while a commit is in flight: `next` then runs once it has landed (registered once, however many wait). `first`:
 * `next` had its turn and let a commit go in (a burst, a drop): it goes again before what waited after it. Not an
 * edit itself: a close that drops it counts the drops still queued (`endDrags`).
 */
function held(first = false): boolean {
  if (commitInFlight() && !waiting) {
    waiting = true;
    afterCommits(
      () => {
        waiting = false;
        next();
      },
      { first, edit: false },
    );
  }
  return commitInFlight();
}

/**
 * `playheadUs`: where the playhead was before the press moved it (a pick seeks), which is what the gesture snaps to.
 */
export function beginDrag(
  e: PointerLike,
  gesture: Gesture,
  view: DragView,
  playheadUs = usePlayback.getState().playheadUs,
) {
  const snapshot = useProject.getState().snapshot;
  if (!snapshot || snapshot.readOnly) return;
  cancelDrag();
  const { main } = snapshot.project.timeline;
  const from = 'clipId' in gesture ? main.findIndex((c) => c.id === gesture.clipId) : -1;
  if ('clipId' in gesture && from < 0) return;
  const s = newSession(e, gesture, view, snapshot, from, playheadUs);
  session = s;
  // Held at the press already; again for a gesture that ended one its release never reached (`cancelDrag` above).
  holdDock();
  stopWatching = watchSession(
    s,
    () => session === s,
    cancelDrag,
    schedule,
    () => outdated(s),
  );
  window.addEventListener('keydown', onKey, true);
  window.addEventListener('keyup', onKey, true);
  // A backstop: a release or cancel the block never hears (it lost the capture) still ends the gesture.
  window.addEventListener('pointerup', endDrag);
  window.addEventListener('pointercancel', onWindowCancel);
}

export function moveDrag(e: PointerLike) {
  const s = session;
  if (!s || e.pointerId !== s.pointerId) return;
  s.last = { x: e.clientX, y: e.clientY, meta: Boolean(e.metaKey) };
  // Past the slop it lifts, unless a commit is in flight: then once that lands (`next`).
  if (!s.started && (!pastSlop(s) || held() || !start(s))) return;
  schedule();
}

/**
 * A change from elsewhere merged mid-drag: the drag stops where it is, puts down what it lifted and drops its dry
 * run, so nothing shows its travel added to where that change put things. Its release then sends the ops it last built,
 * on the copy it began on, and Rust refuses them: "The video changed. Try again."
 */
function outdated(s: Session) {
  s.stale = true;
  if (s.sent) clearPreview();
  s.sent = null;
  unshowEdge(s);
  lower(s, false);
}

/** The block lifts and the lanes keep the moving blocks mounted. False: it can't move (a locked track, no project). */
function lift(s: Session): boolean {
  const g = s.gesture;
  const project = useProject.getState().snapshot?.project;
  if (!project) return false;
  if ('itemId' in g && findItem(project, g.itemId)?.track.locked) {
    toast.info(t({ id: 'timeline.trackLocked', message: 'This track is locked.' }));
    return false;
  }
  s.started = true;
  const id = draggedId(g);
  const attached = g.kind === 'clip' ? (attachedTo(project.timeline.tracks).get(id) ?? NONE) : NONE;
  setLifted([id, ...attached], g.kind === 'item' ? g.trackKind : null);
  return true;
}

/** Past the slop: it lifts and autoscroll runs. */
function start(s: Session): boolean {
  if (!lift(s)) {
    finish();
    return false;
  }
  s.stopScroll = startAutoscroll(s.view.scroller, gutterPx(), () => s.last.x, frame);
  return true;
}

function schedule() {
  if (scheduled) return;
  scheduled = true;
  nextFrame(() => {
    scheduled = false;
    frame();
  });
}

/** The live drag where the pointer is now, and its dry run: once per animation frame. */
function frame() {
  const s = session;
  if (!s?.started || s.stale) return;
  if (!draw(s)) return cancelDrag(); // the block is gone
  // Playing, the dry run is what the commit will be; paused, a trim's may leave out a transition (`dryOps`).
  const dry = usePlayback.getState().playing ? s.ops : (s.dryOps ?? s.ops);
  if (dry.length === 0) {
    if (s.sent) clearPreview();
    s.sent = null;
  } else if (!s.sent || !sameOps(s.sent, dry)) {
    // Each dry run re-applies, re-plans and rebuilds the sound in the engine: only a change is sent.
    previewOps(dry);
    s.sent = dry;
  }
  showEdge(s);
}

/**
 * A trim shows the frame at the edge it drags (`s.edgeUs`, in its dry run), not the playhead's; the playhead and the
 * timecode stay where they are. Not while playing: the picture is playback's then.
 */
function showEdge(s: Session) {
  if (usePlayback.getState().playing) s.shownUs = null;
  else if (s.edgeUs !== null && s.edgeUs !== s.shownUs) {
    s.shownUs = s.edgeUs;
    showFrameAt(s.edgeUs);
  }
}

/** The trim ended or stopped: the preview shows the playhead's frame again (playing, it already does). */
function unshowEdge(s: Session) {
  if (s.shownUs === null) return;
  s.shownUs = null;
  if (!usePlayback.getState().playing) showFrameAt(null);
}

/** Draws `s` from the project as it is now, and builds its ops. False: its block is gone. */
function draw(s: Session): boolean {
  const project = useProject.getState().snapshot?.project;
  const dxPx = s.last.x - s.origin.x + ((s.scrollAt ?? s.view.scroller.scrollLeft) - s.origin.scroll);
  const dtUs = pxToUs(dxPx, s.pps);
  const now = new Set<string>();
  const g = s.gesture;
  const ok =
    project !== undefined &&
    (g.kind === 'clip'
      ? frameClip(s, g, project, dxPx, dtUs, now)
      : g.kind === 'item'
        ? frameItem(s, g, project, dtUs, now)
        : frameTrim(s, g, project, dtUs, now));
  if (!ok) return false;
  for (const id of s.shifted) if (!now.has(id)) shiftBlock(id, 0);
  s.shifted = now;
  // A block remounted under the drag (its lane re-rendered) lifts again.
  const el = blockElement(draggedId(g));
  if (el && el.dataset.dragging !== 'true') el.dataset.dragging = 'true';
  return true;
}

/** Release. A drop made while a commit is in flight waits for it (see `queued`). */
export function endDrag(e: PointerLike) {
  const s = session;
  if (!s || e.pointerId !== s.pointerId) return;
  s.last = { x: e.clientX, y: e.clientY, meta: Boolean(e.metaKey) };
  if (!s.started && !pastSlop(s)) return finish(); // a click: the press already selected
  detach(s);
  s.scrollAt = s.view.scroller.scrollLeft;
  if (!held()) return commit(s);
  // The row it was dropped on, by its track's id: the commit in flight may add a track and move the rows.
  if (s.gesture.kind === 'item') s.dropRow = s.view.rowAt(s.last.y, s.gesture.trackKind);
  queued.push(s);
}

/**
 * The final frame, from the project as it is now, then one transaction (one undo step) through the same `apply` chat
 * and plugins use. When it settles, the next queued drop commits.
 */
function commit(s: Session): void {
  if (!s.started && !lift(s)) return next();
  // Outdated: what it built before the change, never redrawn on the changed project (its base makes Rust refuse it).
  const drawn = s.stale || draw(s);
  const committing = drawn && s.ops.length > 0 && s.label !== null;
  const id = draggedId(s.gesture);
  const focused = document.activeElement !== null && document.activeElement === blockElement(id);
  // The blocks stay where the drag left them until the new revision draws them there.
  lower(s, committing);
  if (!committing) {
    if (s.sent) clearPreview();
    return next();
  }
  const { ops, said } = s;
  const label = s.label!;
  dropPendingPreview();
  const end = beginCommit();
  const done = (ok: boolean) => {
    try {
      if (ok) announce(said);
      else clearPreview();
      // A store change renders in a microtask queued before this one: the blocks are already drawn where they now
      // are, so the shifts go in the same frame (no frame drawn twice as far).
      clearShifts();
      setLifted(NONE, null);
      // A moved block can remount (another lane) or move in the DOM: focus stays on it rather than dropping to body.
      const active = document.activeElement;
      if (focused && (active === null || active === document.body))
        blockElement(id)?.focus({ preventScroll: true });
    } finally {
      end(); // what waits for it (the next drop, a held press, a keyboard edit) runs, whatever happened above
    }
  };
  // eslint-disable-next-line no-restricted-syntax -- a drop sent at its turn (`held`, `next`), holding the commit queue
  void useProject
    .getState()
    .apply(label, ops, undefined, s.built)
    .then(done, () => done(false))
    .catch((error: unknown) => console.error(error));
}

/** Once no commit is in flight: the next queued drop commits, else a press held past its slop lifts. */
function next(): void {
  // A nudge a waiting press left settling goes in first: the drop or the lift builds on where it put things.
  if (keyBurst.flush()) return void held(true);
  const s = queued.shift();
  if (s) {
    commit(s);
    if (queued.length > 0 || session) held(true);
    return;
  }
  const live = session;
  if (live && !live.started && pastSlop(live) && start(live)) frame();
}

/**
 * The project closes: the live gesture is cancelled, and drops still queued are dropped (never sent); returns how many,
 * for the close to say. It first waits for them in their turn: it drops only those its bound gave up on. (The timeline
 * going while the project stays open ends only the gesture, `cancelDrag`: its drops go in.)
 */
export function endDrags(): number {
  cancelDrag();
  waiting = false;
  const dropped = queued.splice(0).length;
  if (dropped === 0) return 0;
  clearShifts();
  setLifted(NONE, null);
  return dropped;
}
onAbandon(endDrags);
// A closing project ends the gesture under way at once; drops already made go in, in their turn, before it closes.
onCloseStart(cancelDrag);

/** Escape, pointercancel, lost capture or a new revision: everything goes back, nothing is committed. */
export function cancelDrag() {
  const s = session;
  if (!s) return;
  finish();
  if (s.sent) clearPreview();
}

function finish() {
  const s = session;
  if (!s) return;
  detach(s);
  if (s.started) lower(s, false);
}

/** The live gesture ends: no pointer, key or store event reaches it any more, and the preview docks again. */
function detach(s: Session) {
  if (session === s) session = null;
  unshowEdge(s);
  releaseDock();
  stopWatching?.();
  stopWatching = null;
  window.removeEventListener('keydown', onKey, true);
  window.removeEventListener('keyup', onKey, true);
  window.removeEventListener('pointerup', endDrag);
  window.removeEventListener('pointercancel', onWindowCancel);
}

function onWindowCancel(e: PointerEvent) {
  if (e.pointerId === session?.pointerId) cancelDrag();
}

function onKey(e: KeyboardEvent) {
  const s = session;
  if (!s || (!s.started && !pastSlop(s))) return;
  if (e.key === 'Escape' && e.type === 'keydown') {
    // Escape ends the drag only: it doesn't also clear the selection.
    e.preventDefault();
    e.stopPropagation();
    cancelDrag();
  } else if (e.key === 'Meta') {
    s.last.meta = e.type === 'keydown';
    schedule();
  }
}
