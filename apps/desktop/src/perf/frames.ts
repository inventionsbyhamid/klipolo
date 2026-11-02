/**
 * A run of frames: how many, the median, the 95th percentile and the longest (ms), and how many came late: over 20 ms
 * (a frame missed at 60 fps, past the jitter of a display's 16.7 ms) and over 50 ms (a hitch a person sees).
 */
export interface FrameStats {
  frames: number;
  median: number;
  p95: number;
  longest: number;
  late: number;
  veryLate: number;
}

const LATE_MS = 20;
const VERY_LATE_MS = 50;

/** The stats of frames `deltas` ms apart. */
export function frameStats(deltas: number[]): FrameStats {
  if (deltas.length === 0) return { frames: 0, median: 0, p95: 0, longest: 0, late: 0, veryLate: 0 };
  const sorted = [...deltas].sort((a, b) => a - b);
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  return {
    frames: deltas.length,
    median: Math.round(at(0.5)),
    p95: Math.round(at(0.95)),
    longest: Math.round(sorted[sorted.length - 1]),
    late: deltas.filter((d) => d > LATE_MS).length,
    veryLate: deltas.filter((d) => d > VERY_LATE_MS).length,
  };
}

/** `stats` as the log says them. */
export function framesLine(s: FrameStats): string {
  return (
    `${s.frames} frames, median ${s.median} ms, p95 ${s.p95} ms, longest ${s.longest} ms, ` +
    `${s.late} over ${LATE_MS} ms, ${s.veryLate} over ${VERY_LATE_MS} ms`
  );
}
