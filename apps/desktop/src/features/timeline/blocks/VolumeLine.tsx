import { t } from '@lingui/core/macro';
import { memo, useRef, type PointerEvent } from 'react';
import type { ItemContent } from '../../../bindings/ItemContent';
import type { RangeUs } from '../../../bindings/RangeUs';
import { commitLatest } from '../../../state/commit';
import type { Op } from '../../../bindings/Op';
import { contentNow } from '../../inspector/latest';
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
 * None when it doesn't duck (no depth, or 0 dB) or nobody speaks while it plays.
 */
export function duckDips(
  durationUs: number,
  startUs: number,
  duck: number | undefined,
  speech: RangeUs[],
): [number, number, number, number][] {
  const dips: [number, number, number, number][] = [];
  if (duck === undefined || !(duck < 0)) return dips;
  for (const r of speech) {
    const from = r.start - startUs;
    const to = r.end - startUs;
    if (to + RELEASE_US <= 0 || from - ATTACK_US >= durationUs) continue;
    dips.push([from - ATTACK_US, from, to, to + RELEASE_US]);
  }
  return dips;
}

/**
 * The music's level across the layer, as the mix plays it: its volume, its fades, and the dips under speech
 * (`duckDips`). Points are (µs from the layer's start, gain), at every bend.
 */
export function levelPoints(
  durationUs: number,
  startUs: number,
  content: Pick<AudioContent, 'volume' | 'fadeInUs' | 'fadeOutUs' | 'duck' | 'muted'>,
  speech: RangeUs[],
): [number, number][] {
  const duck = content.duck !== undefined && content.duck < 0 ? 10 ** (content.duck / 20) : 1;
  const volume = content.muted ? 0 : content.volume;
  const dips = duckDips(durationUs, startUs, content.duck, speech);
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
  return [...bends]
    .filter((us) => us >= 0 && us <= durationUs)
    .sort((a, b) => a - b)
    .map((us) => [us, volume * fadeAt(us) * duckAt(us)]);
}

interface VolumeLineProps {
  id: string;
  startUs: number;
  durationUs: number;
  content: AudioContent;
  pps: number;
  height: number;
}

/** A piece narrower than this draws no "Quieter while people talk" tag: it would cover the line. */
const TAG_MIN_PX = 240;

/**
 * The Music lane's volume line: violet, 2 px, at the music's level, dipping where it ducks under speech,
 * with "Quieter while people talk" at the right of each piece wide enough to hold it. Drag the line up or down to
 * change the volume; Option drag to change how deep it ducks. Each drag is one undo step, committed on release.
 */
export const VolumeLine = memo(function VolumeLine({
  id,
  startUs,
  durationUs,
  content,
  pps,
  height,
}: VolumeLineProps) {
  const speech = useSpeech((s) => s.ranges);
  const press = useRef<{ y: number; volume: number; duck: number; deep: boolean; pointerId: number } | null>(null);
  const line = useRef<SVGPolylineElement>(null);
  const width = Math.max(1, usToPx(durationUs, pps));
  const points = levelPoints(durationUs, startUs, content, speech);
  // Tagged where it ducks under someone speaking: its fades alone are no dip.
  const dips = duckDips(durationUs, startUs, content.duck, speech).length > 0;
  const drawn = (pts: [number, number][]) =>
    pts.map(([us, g]) => `${((us / durationUs) * width).toFixed(1)},${(levelY(g) * height).toFixed(1)}`).join(' ');
  const shown = drawn(points);

  const onPointerDown = (e: PointerEvent<SVGPolylineElement>) => {
    if (e.button !== 0) return;
    // The block's own drag (a move) never starts from the line.
    e.stopPropagation();
    e.preventDefault();
    e.currentTarget.setPointerCapture?.(e.pointerId);
    press.current = {
      y: e.clientY,
      volume: content.volume,
      duck: content.duck ?? 0,
      deep: e.altKey && content.duck !== undefined,
      pointerId: e.pointerId,
    };
  };
  /** Where the drag puts the volume (or the ducking depth, Option held). */
  const dragged = (clientY: number) => {
    const p = press.current!;
    const dy = (clientY - p.y) / Math.max(1, height);
    if (p.deep) {
      const duck = Math.round(Math.min(0, Math.max(DEEPEST_DUCK_DB, p.duck - dy * RANGE_DB)) * 2) / 2;
      return { volume: p.volume, duck };
    }
    const volume = Math.round(levelAt(levelY(p.volume) + dy) * 100) / 100;
    return { volume, duck: p.duck };
  };
  const onPointerMove = (e: PointerEvent<SVGPolylineElement>) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    const next = dragged(e.clientY);
    // Drawn straight onto the line: no render per move.
    line.current?.setAttribute(
      'points',
      drawn(levelPoints(durationUs, startUs, { ...content, volume: next.volume, duck: next.duck }, speech)),
    );
  };
  const onPointerUp = (e: PointerEvent<SVGPolylineElement>) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    const next = dragged(e.clientY);
    press.current = null;
    // Built at its turn on the music as the project has it then: a fade changed meanwhile stays.
    const set = (patch: Partial<AudioContent>): Op[] => [
      { type: 'updateItem', id, patch: { content: { ...contentNow(id, content), ...patch } } },
    ];
    // A change that didn't go in (refused, or the music gone) leaves the line where the project has it.
    const back = (ok: boolean) => {
      if (!ok) line.current?.setAttribute('points', shown);
    };
    if (p.deep && next.duck !== p.duck) void commitLatest('music.duck.undo', () => set({ duck: next.duck })).then(back);
    else if (!p.deep && next.volume !== p.volume)
      void commitLatest('volume.undo', () => set({ volume: next.volume })).then(back);
    else back(false);
  };
  return (
    <>
      <svg
        className="tl-volume"
        width={width}
        height={height}
        viewBox={`0 0 ${width} ${height}`}
        aria-hidden
        data-testid="volume-line"
      >
        <polyline ref={line} className="tl-volume-line" points={shown} />
        <polyline
          className="tl-volume-grip"
          points={shown}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={() => {
            press.current = null;
            line.current?.setAttribute('points', shown);
          }}
        >
          <title>
            {t({
              id: 'music.line.tip',
              message: 'Drag to change the volume. Option drag a dip to change how much quieter it gets.',
              comment: 'Tooltip of the Music lane’s volume line',
            })}
          </title>
        </polyline>
      </svg>
      {dips && width >= TAG_MIN_PX && (
        // Across the piece, so the tag keeps to the right edge of what the lanes show of it.
        <span className="tl-duck-band">
          <span className="tl-duck-tag">
            {t({
              id: 'music.ducking.tag',
              message: 'Quieter while people talk',
              comment: 'On the Music lane, where the music ducks under speech',
            })}
          </span>
        </span>
      )}
    </>
  );
});
