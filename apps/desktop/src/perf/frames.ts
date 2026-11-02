/** A run of frames: how many, the median, the 95th percentile and the longest (ms), and how many missed 60 and 30 fps. */
export interface FrameStats {
  frames: number;
  median: number;
  p95: number;
  longest: number;
  over16: number;
  over33: number;
}

/** The stats of frames `deltas` ms apart. */
export function frameStats(deltas: number[]): FrameStats {
  if (deltas.length === 0) return { frames: 0, median: 0, p95: 0, longest: 0, over16: 0, over33: 0 };
  const sorted = [...deltas].sort((a, b) => a - b);
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  return {
    frames: deltas.length,
    median: Math.round(at(0.5)),
    p95: Math.round(at(0.95)),
    longest: Math.round(sorted[sorted.length - 1]),
    over16: deltas.filter((d) => d > 1000 / 60).length,
    over33: deltas.filter((d) => d > 1000 / 30).length,
  };
}

/** `stats` as the log says them. */
export function framesLine(s: FrameStats): string {
  return (
    `${s.frames} frames, median ${s.median} ms, p95 ${s.p95} ms, longest ${s.longest} ms, ` +
    `${s.over16} over 16.7 ms, ${s.over33} over 33 ms`
  );
}
