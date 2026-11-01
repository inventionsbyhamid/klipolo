import type { Ease } from '../bindings/Ease';
import type { Key } from '../bindings/Key';
import type { Motion } from '../bindings/Motion';
import type { MotionKeys } from '../bindings/MotionKeys';

/*
 * Keys set by hand: a layer's motion keys (`klipolo_model::motion`) and a clip's or sound's volume keys
 * (`klipolo_model::volume`). One key list and one evaluator, the model's, so the canvas and the volume line draw what the
 * engine plays.
 */

/** At most this many keys per property (the model's `MAX_KEYS`). */
export const MAX_KEYS = 64;

/** The values each motion property allows (the model's `LIMITS`), and a volume key's. */
export const LIMITS = {
  x: [-4, 4],
  y: [-4, 4],
  scale: [0, 20],
  rotation: [-3600, 3600],
  opacity: [0, 1],
} as const satisfies Record<keyof MotionKeys, readonly [number, number]>;
export const VOLUME_LIMITS = [0, 2] as const;

/** Each motion property's value with no keys (the model's `MotionKeys::each`). */
export const REST: Record<keyof MotionKeys, number> = { x: 0, y: 0, scale: 1, rotation: 0, opacity: 1 };

/** The spring the card's Spring ease sets: a little overshoot, then settled (damping ratio 0.5). */
export const SPRING: Ease = { spring: { stiffness: 100, damping: 10, mass: 1 } };

const C1 = 1.70158;
const C2 = C1 * 1.525;
const C3 = C1 + 1;
const C4 = (2 * Math.PI) / 3;
const C5 = (2 * Math.PI) / 4.5;

const powerIn = (n: number, t: number) => t ** n;
const powerOut = (n: number, t: number) => 1 - (1 - t) ** n;
const powerInOut = (n: number, t: number) => (t < 0.5 ? (2 * t) ** n / 2 : 1 - (2 - 2 * t) ** n / 2);

function bounceOut(t: number): number {
  const n1 = 7.5625;
  const d1 = 2.75;
  if (t < 1 / d1) return n1 * t * t;
  if (t < 2 / d1) return n1 * (t - 1.5 / d1) ** 2 + 0.75;
  if (t < 2.5 / d1) return n1 * (t - 2.25 / d1) ** 2 + 0.9375;
  return n1 * (t - 2.625 / d1) ** 2 + 0.984375;
}

/** CSS `cubic-bezier` at `t`: Newton's method on x, then bisection if it stalls (the model's). */
function bezier(x1: number, y1: number, x2: number, y2: number, t: number): number {
  const curve = (a: number, b: number, s: number) => 3 * a * s * (1 - s) ** 2 + 3 * b * s * s * (1 - s) + s ** 3;
  const slope = (a: number, b: number, s: number) =>
    3 * a * (1 - s) ** 2 + 6 * (b - a) * s * (1 - s) + 3 * (1 - b) * s * s;
  let s = t;
  for (let i = 0; i < 8; i++) {
    const err = curve(x1, x2, s) - t;
    if (Math.abs(err) < 1e-7) return curve(y1, y2, s);
    const d = slope(x1, x2, s);
    if (Math.abs(d) < 1e-6) break;
    s = Math.min(1, Math.max(0, s - err / d));
  }
  let [lo, hi] = [0, 1];
  s = t;
  for (let i = 0; i < 30; i++) {
    const x = curve(x1, x2, s);
    if (Math.abs(x - t) < 1e-7) break;
    if (x < t) lo = s;
    else hi = s;
    s = (lo + hi) / 2;
  }
  return curve(y1, y2, s);
}

/** A spring released from rest, stretched so the key's span is its settle time (the model's `spring` module). */
function spring(k: number, c: number, m: number, t: number): number {
  if (t >= 1) return 1;
  const ratio = c / (2 * Math.sqrt(k * m));
  const zeta = Number.isNaN(ratio) ? 0.05 : Math.min(160, Math.max(0.05, ratio));
  const step = (u: number) => {
    if (zeta < 1) {
      const wd = Math.sqrt(1 - zeta * zeta);
      return 1 - Math.exp(-zeta * u) * (Math.cos(wd * u) + (zeta / wd) * Math.sin(wd * u));
    }
    if (zeta === 1) return 1 - Math.exp(-u) * (1 + u);
    const r = Math.sqrt(zeta * zeta - 1);
    const [a, b] = [-1 / (zeta + r), -(zeta + r)];
    return 1 - (b * Math.exp(a * u) - a * Math.exp(b * u)) / (b - a);
  };
  const settleLn = 3 * Math.LN10;
  const end = zeta <= 1 ? settleLn / zeta : settleLn * (zeta + Math.sqrt(zeta * zeta - 1));
  return step(t * end) / step(end);
}

/** Progress `t` (0 to 1) through `ease`: the model's `Ease::apply`. */
export function easeAt(ease: Ease | undefined, t: number): number {
  t = Math.min(1, Math.max(0, t));
  const e = ease ?? 'linear';
  if (typeof e === 'object') {
    if ('cubicBezier' in e) return bezier(e.cubicBezier.x1, e.cubicBezier.y1, e.cubicBezier.x2, e.cubicBezier.y2, t);
    if ('steps' in e) {
      if (t >= 1) return 1;
      const n = Math.max(1, e.steps.n);
      return Math.floor(t * n) / n;
    }
    return spring(e.spring.stiffness, e.spring.damping, e.spring.mass, t);
  }
  switch (e) {
    case 'linear':
      return t;
    case 'ease':
      return bezier(0.25, 0.1, 0.25, 1, t);
    case 'easeIn':
      return bezier(0.42, 0, 1, 1, t);
    case 'easeOut':
      return bezier(0, 0, 0.58, 1, t);
    case 'easeInOut':
      return bezier(0.42, 0, 0.58, 1, t);
    case 'power1In':
      return powerIn(2, t);
    case 'power1Out':
      return powerOut(2, t);
    case 'power1InOut':
      return powerInOut(2, t);
    case 'power2In':
      return powerIn(3, t);
    case 'power2Out':
      return powerOut(3, t);
    case 'power2InOut':
      return powerInOut(3, t);
    case 'power3In':
      return powerIn(4, t);
    case 'power3Out':
      return powerOut(4, t);
    case 'power3InOut':
      return powerInOut(4, t);
    case 'power4In':
      return powerIn(5, t);
    case 'power4Out':
      return powerOut(5, t);
    case 'power4InOut':
      return powerInOut(5, t);
    case 'backIn':
      return C3 * t ** 3 - C1 * t * t;
    case 'backOut':
      return 1 + C3 * (t - 1) ** 3 + C1 * (t - 1) ** 2;
    case 'backInOut':
      return t < 0.5
        ? ((2 * t) ** 2 * ((C2 + 1) * 2 * t - C2)) / 2
        : ((2 * t - 2) ** 2 * ((C2 + 1) * (2 * t - 2) + C2) + 2) / 2;
    case 'elasticIn':
      return t === 0 || t === 1 ? t : -(2 ** (10 * t - 10)) * Math.sin((10 * t - 10.75) * C4);
    case 'elasticOut':
      return t === 0 || t === 1 ? t : 2 ** (-10 * t) * Math.sin((10 * t - 0.75) * C4) + 1;
    case 'elasticInOut':
      if (t === 0 || t === 1) return t;
      return t < 0.5
        ? -(2 ** (20 * t - 10) * Math.sin((20 * t - 11.125) * C5)) / 2
        : (2 ** (-20 * t + 10) * Math.sin((20 * t - 11.125) * C5)) / 2 + 1;
    case 'bounceIn':
      return 1 - bounceOut(1 - t);
    case 'bounceOut':
      return bounceOut(t);
    case 'bounceInOut':
      return t < 0.5 ? (1 - bounceOut(1 - 2 * t)) / 2 : (1 + bounceOut(2 * t - 1)) / 2;
    case 'sineIn':
      return t === 1 ? 1 : 1 - Math.cos((t * Math.PI) / 2);
    case 'sineOut':
      return Math.sin((t * Math.PI) / 2);
    case 'sineInOut':
      return -(Math.cos(Math.PI * t) - 1) / 2;
    case 'expoIn':
      return t === 0 ? 0 : 2 ** (10 * t - 10);
    case 'expoOut':
      return t === 1 ? 1 : 1 - 2 ** (-10 * t);
    case 'expoInOut':
      if (t === 0 || t === 1) return t;
      return t < 0.5 ? 2 ** (20 * t - 10) / 2 : (2 - 2 ** (-20 * t + 10)) / 2;
    case 'circIn':
      return 1 - Math.sqrt(1 - t * t);
    case 'circOut':
      return Math.sqrt(1 - (t - 1) ** 2);
    case 'circInOut':
      return t < 0.5 ? (1 - Math.sqrt(1 - (2 * t) ** 2)) / 2 : (Math.sqrt(1 - (-2 * t + 2) ** 2) + 1) / 2;
    case 'hold':
      return t >= 1 ? 1 : 0;
  }
}

/** One property at `tUs`: the first key's value before it, the last's after it, eased in between; `rest` with none. */
export function valueAt(keys: readonly Key[] | undefined, tUs: number, rest: number): number {
  if (!keys || keys.length === 0) return rest;
  if (tUs <= keys[0].tUs) return keys[0].v;
  const next = keys.findIndex((k) => k.tUs > tUs);
  if (next < 0) return keys[keys.length - 1].v;
  const [a, b] = [keys[next - 1], keys[next]];
  const span = Math.max(1, b.tUs - a.tUs);
  return a.v + (b.v - a.v) * easeAt(b.ease, (tUs - a.tUs) / span);
}

/** A clip's or sound's level at `tUs` from its start: its line within 0 to 2, else its volume. */
export function levelAt(keys: readonly Key[] | undefined, tUs: number, volume: number): number {
  if (!keys || keys.length === 0) return volume;
  return Math.min(VOLUME_LIMITS[1], Math.max(VOLUME_LIMITS[0], valueAt(keys, tUs, volume)));
}

/** The index of the key within `slackUs` of `tUs` (the nearest), or -1. */
export function keyIndexAt(keys: readonly Key[] | undefined, tUs: number, slackUs: number): number {
  let best = -1;
  keys?.forEach((k, i) => {
    const off = Math.abs(k.tUs - tUs);
    if (off <= slackUs && (best < 0 || off < Math.abs(keys[best].tUs - tUs))) best = i;
  });
  return best;
}

/**
 * `keys` with a key of `v` at `tUs` (replacing one within `slackUs`, which keeps its time and ease unless `ease` is
 * given), in order, at most `MAX_KEYS`; null when it would be one too many.
 */
export function withKey(keys: readonly Key[] | undefined, tUs: number, v: number, slackUs = 0, ease?: Ease): Key[] | null {
  const list = [...(keys ?? [])];
  const at = keyIndexAt(list, tUs, slackUs);
  if (at >= 0) {
    list[at] = { ...list[at], v, ...(ease !== undefined ? { ease } : {}) };
    return list;
  }
  if (list.length >= MAX_KEYS) return null;
  const key: Key = ease !== undefined && ease !== 'linear' ? { tUs, v, ease } : { tUs, v };
  const i = list.findIndex((k) => k.tUs > tUs);
  list.splice(i < 0 ? list.length : i, 0, key);
  return list;
}

/** `keys` without the key within `slackUs` of `tUs`. */
export function withoutKey(keys: readonly Key[] | undefined, tUs: number, slackUs = 0): Key[] {
  const list = [...(keys ?? [])];
  const at = keyIndexAt(list, tUs, slackUs);
  if (at >= 0) list.splice(at, 1);
  return list;
}

/** `keys` with the key at `fromUs` moved to `toUs` (within `[0, lenUs]`, and never onto another key: it stops 1 µs
 * short of its neighbours); a `copy` also keeps the key at `fromUs`. Unchanged when no key is at `fromUs`. */
export function movedKey(keys: readonly Key[], fromUs: number, toUs: number, lenUs: number, copy = false): Key[] {
  const at = keys.findIndex((k) => k.tUs === fromUs);
  if (at < 0) return [...keys];
  const key = keys[at];
  const rest = copy ? [...keys] : keys.filter((_, i) => i !== at);
  let t = Math.round(Math.min(lenUs, Math.max(0, toUs)));
  // A key already there: the move stops just short of it, on the side it came from.
  while (rest.some((k) => k.tUs === t)) t += t < fromUs ? 1 : -1;
  if (t < 0 || t > lenUs) return [...keys];
  const out = [...rest, { ...key, tUs: t }];
  return out.sort((a, b) => a.tUs - b.tUs);
}

/** The time of the key before `tUs` (more than `slackUs` earlier), or null. */
export function previousKey(times: readonly number[], tUs: number, slackUs: number): number | null {
  let found: number | null = null;
  for (const t of times) if (t < tUs - slackUs && (found === null || t > found)) found = t;
  return found;
}

/** The time of the key after `tUs` (more than `slackUs` later), or null. */
export function nextKey(times: readonly number[], tUs: number, slackUs: number): number | null {
  let found: number | null = null;
  for (const t of times) if (t > tUs + slackUs && (found === null || t < found)) found = t;
  return found;
}

/** What a layer's motion keys say at `tUs` into it (the model's `motion::sample`). */
export interface MotionSample {
  dx: number;
  dy: number;
  scale: number;
  rotation: number;
  opacity: number;
}

export function sampleMotion(motion: Motion | undefined, tUs: number, lenUs: number): MotionSample {
  const k = motion?.keys;
  const t = Math.min(Math.max(0, lenUs), Math.max(0, tUs));
  return {
    dx: valueAt(k?.x, t, 0),
    dy: valueAt(k?.y, t, 0),
    scale: valueAt(k?.scale, t, 1),
    rotation: valueAt(k?.rotation, t, 0),
    opacity: valueAt(k?.opacity, t, 1),
  };
}

/** The properties the card keys: Position moves x and y together. */
export type MotionProperty = 'position' | 'scale' | 'rotation' | 'opacity';
export const PROPERTY_KEYS: Record<MotionProperty, (keyof MotionKeys)[]> = {
  position: ['x', 'y'],
  scale: ['scale'],
  rotation: ['rotation'],
  opacity: ['opacity'],
};

/** Every time `prop` has a key at (x's and y's together for Position), in order, without repeats. */
export function propertyTimes(motion: Motion | undefined, prop: MotionProperty): number[] {
  const keys = motion?.keys;
  const times = new Set<number>();
  for (const name of PROPERTY_KEYS[prop]) for (const k of keys?.[name] ?? []) times.add(k.tUs);
  return [...times].sort((a, b) => a - b);
}

/** Whether `prop` has any keys. */
export function isKeyed(motion: Motion | undefined, prop: MotionProperty): boolean {
  return PROPERTY_KEYS[prop].some((name) => (motion?.keys?.[name]?.length ?? 0) > 0);
}

/** Every time any property has a key at, in order, without repeats (the timeline's diamonds). */
export function motionKeyTimes(motion: Motion | undefined): number[] {
  const keys = motion?.keys;
  if (!keys) return [];
  const times = new Set<number>();
  for (const name of Object.keys(REST) as (keyof MotionKeys)[]) for (const k of keys[name] ?? []) times.add(k.tUs);
  return [...times].sort((a, b) => a - b);
}

/**
 * `motion` with its keys changed by `change` (each property's list, as it is or empty), keeping its presets; undefined
 * when nothing is left, as the model keeps a layout's motion (`motion::kept`).
 */
export function withMotionKeys(
  motion: Motion | undefined,
  change: (keys: MotionKeys) => Partial<MotionKeys>,
): Motion | undefined {
  const now: MotionKeys = { x: [], y: [], scale: [], rotation: [], opacity: [], ...motion?.keys };
  const keys: MotionKeys = { ...now, ...change(now) };
  const empty = (Object.keys(REST) as (keyof MotionKeys)[]).every((name) => keys[name].length === 0);
  const next: Motion = { ...motion };
  if (empty) delete next.keys;
  else next.keys = keys;
  return next.enter ?? next.exit ?? next.loop ?? next.keys ? next : undefined;
}

/** Each property's keys moved by `change` at the same time `fromUs` (x and y, or every property): a diamond's move. */
export function mapKeysAt(
  motion: Motion | undefined,
  names: readonly (keyof MotionKeys)[],
  change: (keys: Key[]) => Key[],
): Motion | undefined {
  return withMotionKeys(motion, (keys) => Object.fromEntries(names.map((name) => [name, change(keys[name])])));
}

/** A value within `[lo, hi]`. */
export function within([lo, hi]: readonly [number, number], v: number): number {
  return Math.min(hi, Math.max(lo, v));
}
