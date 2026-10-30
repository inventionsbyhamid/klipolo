/** Buckets per second at each peaks level (48 kHz, 256 frames per level-0 bucket, ×4 per level). */
const RATE = [187.5, 46.875, 11.71875, 2.9296875];

/** The coarsest level with at least one bucket per device pixel (finer levels for close zoom). */
export function peakLevel(pps: number, dpr: number): number {
  const need = pps * dpr;
  for (let level = RATE.length - 1; level > 0; level--) if (RATE[level] >= need) return level;
  return 0;
}

/** Buckets per second of source at `level` of a peaks file with `bucket` frames (48 kHz) per level-0 bucket. */
export function bucketRate(bucket: number, level: number): number {
  return 48_000 / (bucket * 4 ** level);
}

export interface WaveMap {
  columns: number;
  bucketsPerColumn: number;
  firstBucket: number;
  height: number;
  /** Volume times the fade envelope at device column `x` (0 for muted). */
  gainAt: (x: number) => number;
  /** The one channel a centred sound plays (`SourceMix.centreFrom`); every channel without it, or past the file's. */
  channel?: number | null;
}

export interface WaveStyle {
  color: string;
  /** Centred (audio layers) or from the bottom (the strip under a clip's filmstrip). */
  baseline: 'centre' | 'bottom';
  /**
   * Heights on a decibel scale from this many dB below full scale (silence) to 0 dBFS (the full height), as ears hear
   * loudness: narration at −20 dBFS fills more than half the strip instead of a few pixels. Linear without it.
   */
  floorDb?: number;
}

/** A column's height (0 to 1) for a sample `level` (0 to 1, gain applied) on `style`'s scale. */
function heightOf(level: number, floorDb: number | undefined): number {
  if (floorDb === undefined) return level;
  return level > 0 ? Math.max(0, 1 + (20 * Math.log10(level)) / floorDb) : 0;
}

/**
 * One filled column per device pixel: the loudest |min| or |max| of its buckets over every channel, or over the one a
 * centred sound plays (`map.channel`: it plays on both sides at that channel's level). A boosted volume (up to 2×)
 * stops at the strip's height; silence and muted blocks draw a 1 px line.
 */
export function drawWaveform(
  ctx: CanvasRenderingContext2D,
  peaks: Int16Array,
  channels: number,
  map: WaveMap,
  style: WaveStyle,
) {
  ctx.fillStyle = style.color;
  const stride = channels * 2;
  const one = map.channel != null && map.channel >= 0 && map.channel < channels;
  const [c0, c1] = one ? [map.channel! * 2, map.channel! * 2 + 2] : [0, stride];
  for (let x = 0; x < map.columns; x++) {
    let peak = 0;
    const from = Math.floor(map.firstBucket + x * map.bucketsPerColumn);
    const to = Math.max(from + 1, Math.floor(map.firstBucket + (x + 1) * map.bucketsPerColumn));
    for (let b = from; b < to; b++)
      for (let c = c0; c < c1; c++) peak = Math.max(peak, Math.abs(peaks[b * stride + c] ?? 0));
    const level = heightOf((peak / 32767) * map.gainAt(x), style.floorDb);
    const h = Math.min(map.height, Math.max(1, Math.round(level * map.height)));
    const y = style.baseline === 'centre' ? Math.round((map.height - h) / 2) : map.height - h;
    ctx.fillRect(x, y, 1, h);
  }
}
