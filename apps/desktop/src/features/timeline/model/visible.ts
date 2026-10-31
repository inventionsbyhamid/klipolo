import type { Spans } from './index';

/** [first, end) of the spans overlapping [fromUs, toUs). */
export function visibleRange(spans: Spans, fromUs: number, toUs: number): [number, number] {
  const { starts, ends } = spans;
  let lo = 0;
  let hi = ends.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (ends[mid] <= fromUs) lo = mid + 1;
    else hi = mid;
  }
  const first = lo;
  hi = starts.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (starts[mid] < toUs) lo = mid + 1;
    else hi = mid;
  }
  return [first, lo];
}

export type Piece =
  | { kind: 'block'; index: number }
  | { kind: 'band'; first: number; last: number; startUs: number; endUs: number };

/**
 * Tiny blocks further apart than this start a new band: a band never paints over empty time wide enough to see
 * (two layers 40 minutes apart on an hour zoomed out), while dense ones still draw as one.
 */
const BAND_GAP_PX = 8;

/** Blocks narrower than `minPx` join their neighbours in one band (an hour zoomed out); `keep` stays a block. */
export function mergeTiny(
  spans: Spans,
  first: number,
  end: number,
  pps: number,
  minPx: number,
  keep: (index: number) => boolean,
): Piece[] {
  const out: Piece[] = [];
  const minUs = (minPx / pps) * 1_000_000;
  const gapUs = (BAND_GAP_PX / pps) * 1_000_000;
  let band: Extract<Piece, { kind: 'band' }> | null = null;
  for (let i = first; i < end; i++) {
    if (spans.ends[i] - spans.starts[i] < minUs && !keep(i)) {
      if (band && spans.starts[i] - band.endUs < gapUs) {
        band.last = i;
        band.endUs = Math.max(band.endUs, spans.ends[i]);
      } else {
        band = { kind: 'band', first: i, last: i, startUs: spans.starts[i], endUs: spans.ends[i] };
        out.push(band);
      }
    } else {
      band = null;
      out.push({ kind: 'block', index: i });
    }
  }
  return out;
}

/** A line's consecutive bands, drawn as one element: from the first's start to the last's end, with its parts. */
export interface BandRun {
  kind: 'bands';
  first: number;
  startUs: number;
  endUs: number;
  /** Each band's [start, end) in µs: the element paints these and leaves the gaps between them empty. */
  parts: [number, number][];
}

/**
 * `pieces` with each run of bands that no block interrupts drawn as one element (`BandRun`). Bands further apart
 * than 8 px stay apart in time (the gaps are left unpainted), but a sparse row zoomed out (a title every 9 px) costs
 * one element, not one per title: expanded mode then draws about as many as compact mode.
 */
export function runBands(pieces: Piece[]): (Extract<Piece, { kind: 'block' }> | BandRun)[] {
  const out: (Extract<Piece, { kind: 'block' }> | BandRun)[] = [];
  let run: BandRun | null = null;
  for (const piece of pieces) {
    if (piece.kind === 'block') {
      run = null;
      out.push(piece);
    } else if (run) {
      run.endUs = Math.max(run.endUs, piece.endUs);
      run.parts.push([piece.startUs, piece.endUs]);
    } else {
      run = { kind: 'bands', first: piece.first, startUs: piece.startUs, endUs: piece.endUs, parts: [] };
      run.parts.push([piece.startUs, piece.endUs]);
      out.push(run);
    }
  }
  return out;
}

/**
 * `pieces` plus the kept blocks outside [first, end), in order: a dragged, selected or focused block stays mounted when
 * the window moves past it (autoscroll, a scroll), so a drag never loses its element and focus never drops to the page.
 */
export function withKept(
  pieces: Piece[],
  count: number,
  first: number,
  end: number,
  kept: ReadonlySet<string>,
  idAt: (index: number) => string,
): Piece[] {
  if (kept.size === 0) return pieces;
  const before: Piece[] = [];
  const after: Piece[] = [];
  for (let i = 0; i < first; i++) if (kept.has(idAt(i))) before.push({ kind: 'block', index: i });
  for (let i = end; i < count; i++) if (kept.has(idAt(i))) after.push({ kind: 'block', index: i });
  return before.length + after.length === 0 ? pieces : [...before, ...pieces, ...after];
}
