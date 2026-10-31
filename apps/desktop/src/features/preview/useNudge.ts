import { t } from '@lingui/core/macro';
import { useRef, useState, type KeyboardEvent } from 'react';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Op } from '../../bindings/Op';
import { fontSizeFor, keepVisible, MIN_SIZE, moveRect, scaleAbout } from '../../lib/geometry';
import { FORMAT_SIZE } from '../../lib/layout';
import { toast } from '../../state/toasts';
import { samePlacement, type Placement } from './gizmoGesture';
import type { GizmoTarget } from './gizmoTargets';
import { useBurst } from './useBurst';

const ARROWS: Record<string, [number, number]> = {
  ArrowLeft: [-1, 0],
  ArrowRight: [1, 0],
  ArrowUp: [0, -1],
  ArrowDown: [0, 1],
};
/** ⌥ + arrow resizes by this share of the canvas. */
const STEP = 0.01;

/**
 * Arrow keys on a focused, selected gizmo: move by 1 format pixel (Shift: 10); ⌥ resizes by 1 % (→ and ↑ grow; on
 * text, ← and → change the wrap width and ↑ and ↓ scale it with its font). A burst of presses is one undo step
 * (`useBurst`), and a burst that ends where it began commits nothing.
 */
export function useNudge(target: GizmoTarget, format: FormatKind) {
  const [placement, setPlacement] = useState<Placement | null>(null);
  /** Where the current burst began. */
  const start = useRef<Placement | null>(null);
  const burst = useBurst();
  const [W, H] = FORMAT_SIZE[format];
  const canvas = { width: W, height: H };

  const resized = (base: Placement, dir: [number, number]): Placement => {
    const r = base.rect;
    const grow = dir[0] || -dir[1];
    const centre = { x: r.x + r.w / 2, y: r.y + r.h / 2 };
    if (target.label === 'text' && dir[0]) {
      const w = Math.max((MIN_SIZE * Math.min(W, H)) / W, r.w + grow * STEP);
      const rect = { ...r, x: centre.x - w / 2, w };
      return { ...base, ...keepVisible(rect, target.boxFor(rect), base.rotation, canvas) };
    }
    const min = (MIN_SIZE * Math.min(W, H)) / Math.min(r.w * W, r.h * H);
    const s = Math.max(min, 1 + (grow * STEP) / (dir[0] ? r.w : r.h));
    const rect = scaleAbout(r, s, centre);
    const fontSize = base.fontSize === null ? null : fontSizeFor(r, rect, base.fontSize);
    const box = target.boxFor(rect, fontSize ?? undefined);
    return { ...base, ...keepVisible(rect, box, base.rotation, canvas), fontSize };
  };

  /**
   * True when the key was a nudge. `from` is where the layer is now; `opsFor` places it, built when the burst commits
   * (and once a frame for its dry run), on the layer as the project has it then: a drag or a + press that went in
   * while the nudge settled is kept, never put back by the layer this press saw.
   */
  const onKeyDown = (
    e: KeyboardEvent<HTMLElement>,
    from: Placement,
    opsFor: (p: Placement) => Op[],
  ): boolean => {
    const dir = ARROWS[e.key];
    if (!dir || !target.movable) return false;
    e.preventDefault(); // ←/→ elsewhere step frames; here they nudge
    if (target.locked) {
      if (!e.repeat) toast.info(t({ id: 'timeline.trackLocked', message: 'This track is locked.' }));
      return true;
    }
    if (!burst.pending()) start.current = from;
    let next: Placement;
    if (e.altKey) {
      next = resized(from, dir);
    } else {
      const step = e.shiftKey ? 10 : 1;
      const dx = (dir[0] * step) / W;
      const dy = (dir[1] * step) / H;
      const moved = keepVisible(
        moveRect(from.rect, dx, dy),
        moveRect(from.box, dx, dy),
        from.rotation,
        canvas,
      );
      next = { ...from, ...moved };
    }
    setPlacement(next);
    const layer = target.label;
    burst.push({
      // "Nudge text", "Nudge image"
      label: { id: 'gizmo.nudge.undo', params: { layer } },
      ops: () => opsFor(next),
      noop: !!start.current && samePlacement(next, start.current),
      done: () => setPlacement((p) => (p === next ? null : p)),
    });
    return true;
  };

  return { placement, onKeyDown, flush: burst.flush };
}
