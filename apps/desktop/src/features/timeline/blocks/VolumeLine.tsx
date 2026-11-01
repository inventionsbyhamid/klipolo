import { t } from '@lingui/core/macro';
import { memo, useCallback, useRef, type KeyboardEvent, type PointerEvent } from 'react';
import type { ItemContent } from '../../../bindings/ItemContent';
import type { RangeUs } from '../../../bindings/RangeUs';
import { commitLatest } from '../../../state/commit';
import type { Op } from '../../../bindings/Op';
import { levelAt as keyLevel, movedKey } from '../../../lib/keys';
import { formatPrecise } from '../../../lib/time';
import { addVolumeKey, deleteVolumeKey, editVolumeKeys, moveVolumeKey, type Leveled } from '../../../state/keyEdits';
import { registerFollower } from '../drag/registry';
import { useSpeech } from '../media/speech';
import { usToPx } from '../scale';

type AudioContent = Extract<ItemContent, { type: 'audio' }>;

/** How the mix eases music down before the words and back up after them (the engine's `ATTACK_US`, `RELEASE_US`). */
export const ATTACK_US = 150_000;
export const RELEASE_US = 400_000;
/** The loudest the line shows (volume 2, +6 dB) at the top of the lane, and the quietest (-36 dB) at its foot. */
const TOP_DB = 6;
const RANGE_DB = 42;
/** The deepest ducking the model takes. */
export const DEEPEST_DUCK_DB = -30;

/** Where a level (a gain, 0 to 2) sits down the lane: 0 at the top, 1 at the foot. */
export function levelY(level: number): number {
  if (!(level > 0)) return 1;
  const db = 20 * Math.log10(level);
  return Math.min(1, Math.max(0, (TOP_DB - db) / RANGE_DB));
}

/** The level at a place down the lane (`levelY`'s inverse), within the volume's 0 to 2. */
export function levelAt(y: number): number {
  const db = TOP_DB - Math.min(1, Math.max(0, y)) * RANGE_DB;
  return Math.min(2, Math.max(0, 10 ** (db / 20)));
}

/**
 * Where the music ducks under speech (`speech`, sorted), in the layer's own time: each dip goes down from `a` to `b`,
 * stays low to `c` and is back up by `d`, easing in over `ATTACK_US` before the words and out over `RELEASE_US` after.
 * None when it doesn't duck (no depth, or 0 dB), is muted (a flat line has no dip), or nobody speaks while it plays.
 */
export function duckDips(
  durationUs: number,
  startUs: number,
  { duck, muted }: Pick<AudioContent, 'duck' | 'muted'>,
  speech: RangeUs[],
): [number, number, number, number][] {
  const dips: [number, number, number, number][] = [];
  if (muted || duck === undefined || !(duck < 0)) return dips;
  for (const r of speech) {
    const from = r.start - startUs;
    const to = r.end - startUs;
    if (to + RELEASE_US <= 0 || from - ATTACK_US >= durationUs) continue;
    dips.push([from - ATTACK_US, from, to, to + RELEASE_US]);
  }
  return dips;
}

/** What a volume line draws: the level (its volume, or its keys), its fades, and for music its ducking. */
export type LineLevel = Pick<AudioContent, 'volume' | 'fadeInUs' | 'fadeOutUs' | 'volumeKeys'> &
  Partial<Pick<AudioContent, 'duck' | 'muted'>>;

/** Points drawn per stretch between two keys whose ease isn't a straight line. */
const EASED_STEPS = 16;

/**
 * The level across the clip or layer, as the mix plays it: its volume or its volume keys' line, its fades, and the dips
 * under speech (`duckDips`). Points are (µs from the start, gain), at every bend (and along an eased stretch).
 */
export function levelPoints(durationUs: number, startUs: number, content: LineLevel, speech: RangeUs[]): [number, number][] {
  const duck = content.duck !== undefined && content.duck < 0 ? 10 ** (content.duck / 20) : 1;
  const keys = content.volumeKeys ?? [];
  const base = (us: number) => (content.muted ? 0 : keyLevel(keys, us, content.volume));
  const dips = duckDips(durationUs, startUs, { duck: content.duck, muted: content.muted }, speech);
  const duckAt = (us: number) => {
    let gain = 1;
    for (const [a, b, c, d] of dips) {
      const g =
        us <= a || us >= d
          ? 1
          : us < b
            ? 1 + ((duck - 1) * (us - a)) / (b - a)
            : us <= c
              ? duck
              : duck + ((1 - duck) * (us - c)) / (d - c);
      gain = Math.min(gain, g);
    }
    return gain;
  };
  const fadeAt = (us: number) => {
    let g = 1;
    if (content.fadeInUs > 0 && us < content.fadeInUs) g = Math.min(g, us / content.fadeInUs);
    const out = durationUs - content.fadeOutUs;
    if (content.fadeOutUs > 0 && us > out) g = Math.min(g, (durationUs - us) / content.fadeOutUs);
    return Math.max(0, g);
  };
  const bends = new Set<number>([0, durationUs, content.fadeInUs, durationUs - content.fadeOutUs]);
  for (const dip of dips) for (const us of dip) bends.add(us);
  keys.forEach((k, i) => {
    bends.add(k.tUs);
    const before = keys[i - 1];
    if (!before || k.ease === undefined || k.ease === 'linear') return;
    for (let n = 1; n < EASED_STEPS; n++) bends.add(before.tUs + ((k.tUs - before.tUs) * n) / EASED_STEPS);
  });
  return [...bends]
    .filter((us) => us >= 0 && us <= durationUs)
    .sort((a, b) => a - b)
    .map((us) => [us, base(us) * fadeAt(us) * duckAt(us)]);
}

interface VolumeLineProps {
  startUs: number;
  durationUs: number;
  content: LineLevel;
  /** Whose keys a click or a point changes: the clip, or the sound layer. */
  target: Leveled;
  /** The ops a drag of the line without keys sets the volume with (music: every piece of the song). */
  volumeOps: (volume: number) => Op[];
  /** The ops an Option drag sets music's ducking depth with; none for other sounds. */
  duckOps?: (duck: number) => Op[];
  pps: number;
  height: number;
  /** Drawn in the clip's blue on the main track, the lane's colour elsewhere. */
  tone?: 'music' | 'clip' | 'sound';
}

/** A piece narrower than this draws no "Quieter while people talk" tag: it would cover the line. */
const TAG_MIN_PX = 240;

/** A press on the line or on one of its points. */
interface Press {
  x: number;
  y: number;
  volume: number;
  duck: number;
  deep: boolean;
  pointerId: number;
  /** The key a point's press moves, by its time. */
  point: number | null;
}

/** A press that travels less than this (CSS px) is a click: it adds a key, not drags the line. */
const CLICK_SLOP = 3;

/**
 * A volume line: 2 px, at the level the mix plays, dipping where music ducks under speech (its tag is
 * `DuckTag`, beside the block), with a point at each volume key. A click on the line adds a key there at its level; a
 * drag of the line changes the volume, or with keys moves the whole line up or down; Option drag changes how deep music
 * ducks. A point drags to another time and level, and Delete takes it away. Each is one undo step, committed on release.
 */
export const VolumeLine = memo(function VolumeLine({
  startUs,
  durationUs,
  content,
  target,
  volumeOps,
  duckOps,
  pps,
  height,
  tone = 'music',
}: VolumeLineProps) {
  const speech = useSpeech((s) => s.ranges);
  const press = useRef<Press | null>(null);
  const line = useRef<SVGPolylineElement>(null);
  const svg = useRef<SVGSVGElement>(null);
  const width = Math.max(1, usToPx(durationUs, pps));
  const keys = content.volumeKeys ?? [];
  const points = levelPoints(durationUs, startUs, content, speech);
  const drawn = (pts: [number, number][]) =>
    pts.map(([us, g]) => `${((us / durationUs) * width).toFixed(1)},${(levelY(g) * height).toFixed(1)}`).join(' ');
  const shown = drawn(points);
  const xOf = (us: number) => (us / durationUs) * width;
  const usAt = (clientX: number) => {
    const left = svg.current?.getBoundingClientRect().left ?? 0;
    return Math.min(durationUs, Math.max(0, Math.round(((clientX - left) / width) * durationUs)));
  };
  /** Each key's level moved by `dy` of the lane (a share of its height), as the line's drag moves it. */
  const shifted = (dy: number) =>
    keys.map((k) => ({ ...k, v: Math.round(levelAt(levelY(k.v) + dy) * 100) / 100 }));

  const onPointerDown = (e: PointerEvent<SVGElement>, point: number | null = null) => {
    if (e.button !== 0) return;
    // The block's own drag (a move) never starts from the line.
    e.stopPropagation();
    e.preventDefault();
    e.currentTarget.setPointerCapture?.(e.pointerId);
    if (point !== null) (e.currentTarget as SVGElement).focus?.({ preventScroll: true });
    press.current = {
      x: e.clientX,
      y: e.clientY,
      volume: content.volume,
      duck: content.duck ?? 0,
      deep: e.altKey && content.duck !== undefined && !!duckOps,
      pointerId: e.pointerId,
      point,
    };
  };
  /** Where the drag puts the volume (or the ducking depth, Option held). */
  const dragged = (p: Press, clientY: number) => {
    const dy = (clientY - p.y) / Math.max(1, height);
    if (p.deep) {
      const duck = Math.round(Math.min(0, Math.max(DEEPEST_DUCK_DB, p.duck - dy * RANGE_DB)) * 2) / 2;
      return { volume: p.volume, duck, dy };
    }
    const volume = Math.round(levelAt(levelY(p.volume) + dy) * 100) / 100;
    return { volume, duck: p.duck, dy };
  };
  /** The keys with the pressed point moved to where the pointer is. */
  const pointMoved = (p: Press, clientX: number, clientY: number) => {
    const was = keys.find((k) => k.tUs === p.point)!;
    const dy = (clientY - p.y) / Math.max(1, height);
    const v = Math.round(levelAt(levelY(was.v) + dy) * 100) / 100;
    const moved = movedKey(keys, p.point!, p.point! + ((clientX - p.x) / width) * durationUs, durationUs);
    return { keys: moved.map((k) => (keys.includes(k) ? k : { ...k, v })), toUs: moved.find((k) => !keys.includes(k))?.tUs ?? p.point!, v };
  };
  const onPointerMove = (e: PointerEvent<SVGElement>) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    // Drawn straight onto the line: no render per move.
    if (p.point !== null) {
      const next = pointMoved(p, e.clientX, e.clientY).keys;
      line.current?.setAttribute('points', drawn(levelPoints(durationUs, startUs, { ...content, volumeKeys: next }, speech)));
      return;
    }
    const next = dragged(p, e.clientY);
    const level =
      keys.length > 0 && !p.deep
        ? { ...content, volumeKeys: shifted(next.dy) }
        : { ...content, volume: next.volume, duck: next.duck };
    line.current?.setAttribute('points', drawn(levelPoints(durationUs, startUs, level, speech)));
  };
  const onPointerUp = (e: PointerEvent<SVGElement>) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    press.current = null;
    // A change that didn't go in (refused, or the clip gone) leaves the line where the project has it.
    const back = (ok: boolean) => {
      if (!ok) line.current?.setAttribute('points', shown);
    };
    const click = Math.abs(e.clientX - p.x) < CLICK_SLOP && Math.abs(e.clientY - p.y) < CLICK_SLOP;
    if (p.point !== null) {
      if (click) return back(false);
      const { toUs, v } = pointMoved(p, e.clientX, e.clientY);
      void moveVolumeKey(target, p.point, toUs, v).then(back);
      return;
    }
    if (click && !p.deep) {
      // A key on the line where it was clicked, at its level there.
      const us = usAt(e.clientX);
      void addVolumeKey(target, us, keyLevel(keys, us, content.volume)).then(back);
      return;
    }
    const next = dragged(p, e.clientY);
    if (p.deep && duckOps && next.duck !== p.duck) void commitLatest('music.duck.undo', () => duckOps(next.duck)).then(back);
    else if (!p.deep && keys.length > 0 && next.dy !== 0)
      void editVolumeKeys('volume.undo', target, (now) =>
        now.map((k) => ({ ...k, v: Math.round(levelAt(levelY(k.v) + next.dy) * 100) / 100 })),
      ).then(back);
    else if (!p.deep && keys.length === 0 && next.volume !== p.volume)
      void commitLatest('volume.undo', () => volumeOps(next.volume)).then(back);
    else back(false);
  };
  const cancel = () => {
    press.current = null;
    line.current?.setAttribute('points', shown);
  };
  const onPointKey = (e: KeyboardEvent<SVGElement>, us: number) => {
    if (e.key !== 'Delete' && e.key !== 'Backspace') return;
    // The key, not the clip or layer.
    e.preventDefault();
    e.stopPropagation();
    void deleteVolumeKey(target, us);
  };
  const base = (us: number) => (content.muted ? 0 : keyLevel(keys, us, content.volume));
  const pointY = (us: number) => {
    const at = points.find(([t]) => t === us);
    return levelY(at ? at[1] : base(us)) * height;
  };
  return (
    <svg
      ref={svg}
      className={`tl-volume is-${tone}`}
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      aria-hidden={keys.length === 0 || undefined}
      data-testid="volume-line"
    >
      <polyline ref={line} className="tl-volume-line" points={shown} />
      <polyline
        className="tl-volume-grip"
        points={shown}
        onPointerDown={(e) => onPointerDown(e)}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={cancel}
      >
        <title>
          {duckOps
            ? t({
                id: 'music.line.tip',
                message: 'Drag to change the volume. Option drag a dip to change how much quieter it gets.',
                comment: 'Tooltip of the Music lane’s volume line',
              })
            : t({
                id: 'volume.line.tip',
                message: 'Click to add a volume key. Drag to change the volume.',
                comment: 'Tooltip of a clip’s or sound’s volume line on the timeline',
              })}
        </title>
      </polyline>
      {keys.map((k) => (
        <circle
          key={k.tUs}
          className="tl-volume-key"
          cx={xOf(k.tUs)}
          cy={pointY(k.tUs)}
          r={4}
          tabIndex={0}
          role="button"
          aria-label={t({
            id: 'volume.key.name',
            message: `Volume key at ${formatPrecise(startUs + k.tUs)}, ${Math.round(k.v * 100)}%`,
            comment: 'A point on a volume line; {0} is a time on the timeline, {1} its level',
          })}
          onPointerDown={(e) => onPointerDown(e, k.tUs)}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={cancel}
          onClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => onPointKey(e, k.tUs)}
        >
          <title>
            {t({
              id: 'volume.key.tip',
              message: 'Drag to move. Delete removes it.',
              comment: 'Tooltip of a volume key’s point on the timeline',
            })}
          </title>
        </circle>
      ))}
    </svg>
  );
});

/**
 * "Quieter while people talk" over a music piece that ducks under someone speaking (its fades alone are no dip), at
 * the right of the piece, or of what the lanes show of it while the piece runs past their edge. It is the block's
 * sibling in the lane, not inside its button, so it keeps to the scroller's visible right; a piece narrower than
 * `TAG_MIN_PX` has none. `left`, `width` and `top` are the block's own, in the lane's px.
 */
export const DuckTag = memo(function DuckTag({
  id,
  startUs,
  durationUs,
  content,
  left,
  width,
  top,
  dimmed,
}: {
  /** The music block's id: a gesture that moves the block moves the tag, and one that trims it hides the tag. */
  id: string;
  startUs: number;
  durationUs: number;
  content: Pick<AudioContent, 'duck' | 'muted'>;
  left: number;
  width: number;
  top: number;
  /** Dimmed as its block is. */
  dimmed: boolean;
}) {
  const speech = useSpeech((s) => s.ranges);
  const ref = useCallback((el: HTMLSpanElement | null) => registerFollower(id, el), [id]);
  if (width < TAG_MIN_PX || duckDips(durationUs, startUs, content, speech).length === 0) return null;
  return (
    <span
      ref={ref}
      className={dimmed ? 'tl-duck-band is-dimmed' : 'tl-duck-band'}
      style={{ left: left + 6, width: width - 12, top: top + 3 }}
    >
      <span className="tl-duck-tag">
        {t({
          id: 'music.ducking.tag',
          message: 'Quieter while people talk',
          comment: 'On the Music lane, where the music ducks under speech',
        })}
      </span>
    </span>
  );
});
