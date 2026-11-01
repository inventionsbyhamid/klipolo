import type { BoxN } from '../bindings/BoxN';
import type { FormatKind } from '../bindings/FormatKind';
import type { Key } from '../bindings/Key';
import type { Layout } from '../bindings/Layout';
import { layoutPatchFor } from './geometry';
import { LIMITS, isKeyed, sampleMotion, withKey, withMotionKeys, within, type MotionSample } from './keys';
import { resolveRect } from './layout';

/*
 * A layer with motion keys is drawn where its keys put it at the playhead (the engine's `plan::motion::apply`): its box
 * moved by the keys' offsets and scaled about its centre, turned by their rotation on top of its own. A gesture on the
 * canvas works on that drawn place; a property with keys takes the change as its key at the playhead, and one without
 * changes the layer itself as before.
 */

/** `rest` (the layer's box in a format) where the keys' `sample` draws it. */
export function shownRect(rest: BoxN, s: MotionSample): BoxN {
  const w = rest.w * s.scale;
  const h = rest.h * s.scale;
  const cx = rest.x + rest.w / 2 + s.dx;
  const cy = rest.y + rest.h / 2 + s.dy;
  return { x: cx - w / 2, y: cy - h / 2, w, h };
}

/** Whether a layer's keys move, size or turn it (opacity keys leave its place alone). */
export function placeKeyed(layout: Layout): boolean {
  const m = layout.motion;
  return isKeyed(m, 'position') || isKeyed(m, 'scale') || isKeyed(m, 'rotation');
}

/** The box and turn a layer is drawn at, `tUs` into it (`lenUs` long), in `format`. */
export function shownPlace(
  layout: Layout,
  tUs: number,
  lenUs: number,
  primary: FormatKind,
  format: FormatKind,
): { rect: BoxN; rotation: number } {
  const rest = resolveRect(layout, primary, format);
  if (!placeKeyed(layout)) return { rect: rest, rotation: layout.rotation };
  const s = sampleMotion(layout.motion, tUs, lenUs);
  return { rect: shownRect(rest, s), rotation: layout.rotation + s.rotation };
}

const near = (a: number, b: number) => Math.abs(a - b) < 1e-9;

/**
 * Layout `now` after a gesture on the canvas `tUs` into the layer: `place` puts the drawn box and turn the gesture
 * ended at on a layout. A property with keys takes its change as a key at the playhead (within `slackUs` of a key
 * there, that key): Position the move of the box's centre, Scale the change of its size, Rotation the turn. One
 * without keys changes the layer's own box and turn, as without any keys.
 */
export function placeWithKeys(
  now: Layout,
  place: (layout: Layout) => Layout,
  tUs: number,
  lenUs: number,
  primary: FormatKind,
  format: FormatKind,
  slackUs: number,
): Layout {
  if (!placeKeyed(now)) return place(now);
  const m = now.motion;
  const s = sampleMotion(m, tUs, lenUs);
  const rest = resolveRect(now, primary, format);
  const shown = shownRect(rest, s);
  const turned = now.rotation + s.rotation;
  // The gesture, on the layout as it is drawn.
  const drawn: Layout = { ...layoutPatchFor(now, shown, format, primary), rotation: turned };
  const placed = place(drawn);
  const next = resolveRect(placed, primary, format);
  const keyed = { position: isKeyed(m, 'position'), scale: isKeyed(m, 'scale'), rotation: isKeyed(m, 'rotation') };
  // Size: the scale key, or the layer's own box at the keys' scale.
  let scale = s.scale;
  let [w, h] = [rest.w, rest.h];
  if (!near(next.w, shown.w) || !near(next.h, shown.h)) {
    if (keyed.scale) {
      const ratio = !near(next.w, shown.w) ? next.w / shown.w : next.h / shown.h;
      scale = within(LIMITS.scale, s.scale * ratio);
    } else if (s.scale > 0) {
      [w, h] = [next.w / s.scale, next.h / s.scale];
    }
  }
  // Place: the position keys, or the layer's own centre under the keys' offsets.
  let [dx, dy] = [s.dx, s.dy];
  let [cx, cy] = [rest.x + rest.w / 2, rest.y + rest.h / 2];
  const [moveX, moveY] = [next.x + next.w / 2 - (shown.x + shown.w / 2), next.y + next.h / 2 - (shown.y + shown.h / 2)];
  if (!near(moveX, 0) || !near(moveY, 0)) {
    if (keyed.position) {
      [dx, dy] = [within(LIMITS.x, s.dx + moveX), within(LIMITS.y, s.dy + moveY)];
    } else {
      [cx, cy] = [cx + moveX, cy + moveY];
    }
  }
  const box = { x: cx - w / 2, y: cy - h / 2, w, h };
  // Unchanged but for rounding: the layer's own box stays exactly as it was.
  const same = near(box.x, rest.x) && near(box.y, rest.y) && near(box.w, rest.w) && near(box.h, rest.h);
  let layout = same ? now : layoutPatchFor(now, box, format, primary);
  // Turn: the rotation key, or the layer's own.
  let rotation = s.rotation;
  const turn = placed.rotation - turned;
  if (!near(turn, 0)) {
    if (keyed.rotation) rotation = within(LIMITS.rotation, s.rotation + turn);
    else layout = { ...layout, rotation: now.rotation + turn };
  }
  // A key where the value changed: at the playhead (the one there, if any), holding the new value.
  const set = (list: Key[], v: number, was: number): Key[] =>
    near(v, was) ? list : (withKey(list, tUs, v, slackUs) ?? list);
  const motion = withMotionKeys(m, (k) => ({
    ...(keyed.position ? { x: set(k.x, dx, s.dx), y: set(k.y, dy, s.dy) } : {}),
    ...(keyed.scale ? { scale: set(k.scale, scale, s.scale) } : {}),
    ...(keyed.rotation ? { rotation: set(k.rotation, rotation, s.rotation) } : {}),
  }));
  return { ...layout, motion };
}

/**
 * Layout `now` with its opacity set to `opacity` as drawn `tUs` into the layer: the opacity key at the playhead when it
 * has opacity keys (the layer's own opacity times the key's), else its own opacity.
 */
export function opacityWithKeys(now: Layout, opacity: number, tUs: number, lenUs: number, slackUs: number): Layout {
  const keys = now.motion?.keys?.opacity;
  if (!keys || keys.length === 0) return { ...now, opacity };
  const own = now.opacity > 0 ? now.opacity : 1;
  const v = within(LIMITS.opacity, opacity / own);
  const t = Math.min(Math.max(0, tUs), lenUs);
  return {
    ...now,
    ...(now.opacity > 0 ? {} : { opacity: 1 }),
    motion: withMotionKeys(now.motion, (k) => ({ opacity: withKey(k.opacity, t, v, slackUs) ?? k.opacity })),
  };
}

/** The opacity a layer is drawn at `tUs` into it: its own times its opacity keys'. */
export function shownOpacity(layout: Layout, tUs: number, lenUs: number): number {
  return layout.opacity * sampleMotion(layout.motion, tUs, lenUs).opacity;
}
