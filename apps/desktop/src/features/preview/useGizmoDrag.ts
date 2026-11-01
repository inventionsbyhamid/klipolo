import { t } from '@lingui/core/macro';
import { useEffect, useRef, useState, type PointerEvent } from 'react';
import type { BoxN } from '../../bindings/BoxN';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Layout } from '../../bindings/Layout';
import type { Op } from '../../bindings/Op';
import { layoutPatchFor, sameRect, snapTargets, type Handle, type Size } from '../../lib/geometry';
import type { Label, LabelId } from '../../lib/labels';
import { dragArea, safeRect } from '../../lib/safeAreas';
import { usePlayback } from '../../state/playback';
import type { Built } from '../../state/editBase';
import { captureBase } from '../../state/project';
import { useSelection } from '../../state/selection';
import { toast } from '../../state/toasts';
import { useUi } from '../../state/ui';
import { setDragging, showGuides } from './dragFeedback';
import { placeAt, samePlacement, type Gesture, type GizmoMode, type Placement } from './gizmoGesture';
import type { GizmoTarget } from './gizmoTargets';
import { boxTransform } from './layerBox';
import { clearPreview, dropPendingPreview, previewOps } from '../../state/livePreview';
import { queueCommit } from './toolbar/toolOps';

/** A press that travels less than this (CSS px) is a click, not a drag. */
const DRAG_SLOP = 3;
/** The cursor a handle keeps while it's dragged (the captured layer would show its own). */
const CURSORS: Record<Handle, string> = {
  nw: 'nwse-resize',
  se: 'nwse-resize',
  ne: 'nesw-resize',
  sw: 'nesw-resize',
  n: 'ns-resize',
  s: 'ns-resize',
  w: 'ew-resize',
  e: 'ew-resize',
};
const ORIGIN = { x: 0, y: 0 };

interface Drag extends Gesture {
  pointerId: number;
  /** A press on a locked track's layer: it selects but doesn't move, and says why once. */
  locked: boolean;
  /** Where the layer is dragged to; null until the press travels past the slop. */
  latest: Placement | null;
  /** The dry-run ops for `latest`, built at most once per frame by `previewOps`. */
  preview: () => Op[];
  /**
   * Where its ops are built: the copy at the press, since the placement it commits is the press's plus the pointer's
   * travel. A change from elsewhere merged mid-drag (chat's "make the logo bigger") makes Rust refuse the release
   * rather than put the layer's old size back.
   */
  built: Built;
}

function modeOf(target: EventTarget): GizmoMode {
  const handle =
    target instanceof Element ? target.closest('[data-handle]')?.getAttribute('data-handle') : null;
  if (!handle) return 'move';
  return handle === 'rotate' ? 'rotate' : { resize: handle as Handle };
}

/** The undo label for a drag on the canvas, with the layer it moved: "Move text", "Resize image", "Rotate image". */
function gestureLabel(verb: 'move' | 'resize' | 'rotate'): LabelId {
  return `gizmo.${verb}.undo`;
}

/**
 * Pointer state machine for one layer: press (pause, select, focus) on the layer or a handle → move, resize or turn
 * → commit one op on release, or cancel (Escape, pointercancel, lost capture, unmount). While dragging nothing
 * re-renders: the element follows the pointer through its style and the engine gets the exact op as a dry run once
 * per frame. `nudged` is where keyboard nudges put the layer, until the project has them.
 */
export function useGizmoDrag(
  target: GizmoTarget,
  others: GizmoTarget[],
  canvas: Size,
  format: FormatKind,
  primary: FormatKind,
  nudged: Placement | null,
) {
  const ref = useRef<HTMLButtonElement>(null);
  const drag = useRef<Drag | null>(null);
  const [dragging, setDraggingState] = useState(false);
  /** A committed drag, shown until the project with it arrives (or the commit fails). */
  const [settled, setSettled] = useState<Placement | null>(null);
  const current: Placement = settled ??
    nudged ?? {
      rect: target.rect,
      box: target.box,
      rotation: target.layout.rotation,
      fontSize: target.fontSize,
    };
  const { box, rotation } = current;

  /**
   * Puts box `r`, turned `deg`, on a layout as the project has it when the change is built: only what the gesture
   * changed from the render it began on (a turn alone leaves the box, and any other format's placement, as they are).
   */
  const placeFor =
    (r: BoxN, deg: number) =>
    (layout: Layout): Layout => {
      const placed = sameRect(r, target.rect) ? layout : layoutPatchFor(layout, r, format, primary);
      return deg === target.layout.rotation ? placed : { ...placed, rotation: deg };
    };
  /** The ops that place the layer at `p`: built when they are called (a dry run's frame, the change's turn). */
  const opsFor = (p: Placement) => target.ops(placeFor(p.rect, p.rotation), p.fontSize ?? undefined);

  /** Draw `p` straight onto the element, or put back what React last drew. */
  const paint = (d: Drag, p: Placement | null) => {
    const el = ref.current;
    if (!el) return;
    if (d.mode === 'move') {
      // A move is a transform alone: no layout, just compositing.
      const px = (v: number) => Math.round(v * 100) / 100;
      const dx = p ? px((p.box.x - d.start.box.x) * canvas.width) : 0;
      const dy = p ? px((p.box.y - d.start.box.y) * canvas.height) : 0;
      el.style.transform = boxTransform(dx, dy, rotation);
      return;
    }
    const b = p?.box ?? box;
    el.style.left = `${b.x * canvas.width}px`;
    el.style.top = `${b.y * canvas.height}px`;
    el.style.width = `${b.w * canvas.width}px`;
    el.style.height = `${b.h * canvas.height}px`;
    el.style.transform = boxTransform(0, 0, p?.rotation ?? rotation);
  };

  /** End the press; true when it was a drag. */
  const finish = (commit: boolean): boolean => {
    const d = drag.current;
    drag.current = null;
    if (!d) return false;
    if (ref.current) ref.current.style.cursor = '';
    if (!d.latest) return false;
    const latest = d.latest;
    setDraggingState(false);
    setDragging(false);
    if (commit && !samePlacement(latest, d.start)) {
      // The settled box renders in this same frame as the drawn one goes, so the layer never jumps.
      dropPendingPreview();
      setSettled(latest);
      const label: Label = {
        id: gestureLabel(typeof d.mode === 'object' ? 'resize' : d.mode),
        params: { layer: target.label },
      };
      // Through the commit queue, built at its turn on the layer as the project has it then, and kept at close: a
      // change to it still on its way (Bold, an opacity) stays, and one pressed straight after builds on the move.
      void queueCommit(label, () => opsFor(latest), d.built, { keep: label }).landed.then((ok) => {
        if (!ok) clearPreview();
        // A quicker second drag may have settled since; it clears its own.
        setSettled((s) => (s === latest ? null : s));
      });
    } else {
      clearPreview();
    }
    paint(d, null);
    return true;
  };

  useEffect(() => {
    const press = drag;
    return () => {
      // Unmounted mid-drag (the playhead left the layer): don't leave its dry run drawn in every frame.
      if (!press.current?.latest) return;
      press.current = null;
      clearPreview();
      setDragging(false);
    };
  }, []);

  const handlers = {
    onPointerDown(e: PointerEvent<HTMLElement>) {
      if (e.button !== 0) return;
      // Keeps the focus given below: WebKit's mousedown would otherwise blur it (buttons aren't mouse-focusable on
      // a Mac), and the focused gizmo is what hears the keys. Clicks still follow.
      e.preventDefault();
      usePlayback.getState().pause();
      useSelection.getState().select(target.id);
      const el = e.currentTarget;
      el.focus({ preventScroll: true });
      finish(false); // a press whose release never arrived
      if (!target.movable && !target.locked) return; // a title template: its place is the frame
      const mode = target.locked ? 'move' : modeOf(e.target);
      el.setPointerCapture?.(e.pointerId);
      if (typeof mode === 'object') el.style.cursor = CURSORS[mode.resize];
      else if (mode === 'rotate') el.style.cursor = 'grabbing';
      const bounds = mode === 'rotate' ? el.getBoundingClientRect() : null;
      const d: Drag = {
        pointerId: e.pointerId,
        mode,
        locked: target.locked,
        start: current,
        picture: target.label === 'image' || target.label === 'video',
        from: { x: e.clientX, y: e.clientY },
        centre: bounds ? { x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 } : ORIGIN,
        snap: null,
        latest: null,
        preview: () => opsFor(d.latest ?? d.start),
        built: captureBase(),
      };
      drag.current = d;
    },
    onPointerMove(e: PointerEvent<HTMLElement>) {
      const d = drag.current;
      if (!d || e.pointerId !== d.pointerId) return;
      if (!d.latest) {
        if (Math.hypot(e.clientX - d.from.x, e.clientY - d.from.y) < DRAG_SLOP) return;
        if (d.locked) {
          drag.current = null; // once per press
          toast.info(t({ id: 'timeline.trackLocked', message: 'This track is locked.' }));
          return;
        }
        setDraggingState(true);
        setDragging(true);
        if (d.mode === 'move') {
          const boxes = others.filter((o) => o.id !== target.id).map((o) => o.box);
          // In 9:16 a drag always snaps to an area: the one chosen, else the last, else all platforms.
          const { safeArea, lastSafeArea } = useUi.getState();
          d.snap = snapTargets(boxes, safeRect(format, dragArea(safeArea, lastSafeArea)));
        }
      }
      const keys = { shift: e.shiftKey, meta: e.metaKey };
      const { at, guides } = placeAt(d, { x: e.clientX, y: e.clientY }, keys, canvas);
      d.latest = at;
      paint(d, at);
      showGuides(guides);
      previewOps(d.preview);
    },
    onPointerUp(e: PointerEvent<HTMLElement>) {
      if (drag.current?.pointerId === e.pointerId) finish(true);
    },
    onPointerCancel: () => finish(false),
    onLostPointerCapture: () => finish(false),
  };
  return { ref, current, opsFor, dragging, handlers, cancel: () => finish(false) };
}
