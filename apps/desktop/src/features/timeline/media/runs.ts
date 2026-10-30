import { usToPx } from '../scale';

/**
 * One file a block draws from, and where on the block's source clock (an asset's own time; a recording's clock after
 * its source's nudge): over [fromUs, toUs) it plays the file from its start, `rate` file µs per source µs (the
 * recording device's drift, `runRate`). An imported file is one run over all of its time at rate 1.
 */
export interface FileRun {
  path: string;
  fromUs: number;
  toUs: number;
  rate: number;
  /** The file's length (µs): a filmstrip never shows past it, and a zoomed-out one groups its frames by it. */
  fileUs: number;
  /** Where in its file the run starts (`SourceRun.fileInUs`: a sound that started before the clock's zero, or after a
   * pause); none: 0. */
  inUs?: number;
  /** The file's sound starts this long after its picture (`Asset.audioLeadUs`); none: 0. */
  leadUs?: number;
}

export type FileRuns = readonly FileRun[];

/** What a block draws: its filmstrip's runs (none: no filmstrip), and each sound's, the voice first. */
export interface BlockRuns {
  film: FileRuns;
  sounds: readonly FileRuns[];
  /** Each sound's volume in its clip's mix, in `sounds`' order (none: each at 1). */
  gains?: readonly number[];
  /**
   * The channel each sound plays centred from (`SourceMix.centreFrom`), in `sounds`' order; null or none: as recorded.
   * Its waveform draws that channel only, as it is heard.
   */
  centres?: readonly (number | null)[];
}

/** A whole file, played as it is from source time 0 (an imported file), its sound `leadUs` after its picture. */
export const wholeFile = (path: string, fileUs = Infinity, leadUs = 0): FileRun => ({
  path,
  fromUs: 0,
  toUs: Infinity,
  rate: 1,
  fileUs,
  ...(leadUs !== 0 && { leadUs }),
});

/** The most a file's sound lead moves it (µs), as Rust's `Asset::lead_us` clamps it. */
export const MAX_LEAD_US = 500_000;

/**
 * Where a file's sound is at its file time `fileUs`: the sound cache, its peaks and its transcript are in the sound's
 * own time, which starts `leadUs` (the asset's `audioLeadUs`, within ±500 ms) after the picture. Every waveform
 * reads its peaks through this one helper: the timeline's blocks and source lanes, and Line up by hand's waves, whose
 * marks are file time (sound time plus the lead).
 */
export const soundAtFileUs = (fileUs: number, leadUs = 0) =>
  fileUs - Math.min(MAX_LEAD_US, Math.max(-MAX_LEAD_US, leadUs));

/** The file time of a moment of its sound: `soundAtFileUs`'s inverse (a loud moment in the peaks, as a mark). */
export const fileAtSoundUs = (soundUs: number, leadUs = 0) =>
  soundUs + (soundUs - soundAtFileUs(soundUs, leadUs));

/** The run's file time (µs) at source time `us`, from where in its file it starts: what its filmstrip shows. */
export const fileUs = (run: FileRun, us: number) => localUs(run, us) + (run.inUs ?? 0);

/** The run's sound time (µs) at source time `us`: what its waveform reads. */
export const soundUs = (run: FileRun, us: number) => soundAtFileUs(fileUs(run, us), run.leadUs);

/** Where a block is: from its left edge, `sourceInUs` of its footage at `speed`, at `pps`. */
export interface BlockClock {
  sourceInUs: number;
  speed: number;
  pps: number;
}

/** The part of the canvas a run draws: CSS px from the block's left edge, [xa, xb). */
export interface RunPart {
  run: FileRun;
  /** Its index in its list (its file's media version is at the same place). */
  index: number;
  xa: number;
  xb: number;
}

/**
 * The block's source time (µs) at timeline time `us`, for a block that starts at `startUs`: what a paint draws there.
 * A separated sound layer's speed is its device clock's rate (`layerRate`), so this is the file time the engine plays
 * (`time::drifted`, pinned by `recording-parity.json` to within Rust's rounding).
 */
export const sourceAtUs = (clock: Pick<BlockClock, 'sourceInUs' | 'speed'>, startUs: number, us: number) =>
  clock.sourceInUs + (us - startUs) * clock.speed;

/** Block-relative CSS px of source time `us`. */
export const xOf = (clock: BlockClock, us: number) =>
  usToPx((us - clock.sourceInUs) / clock.speed, clock.pps);

/**
 * The runs' parts inside the canvas's span [x0, x1) (block-relative CSS px), in order. Sound plays nothing between
 * runs; pictures (`hold`) hold the nearest run's frame across a gap, as the engine's `run_or_nearest` does: each run
 * reaches halfway to its neighbours, the first and last without end. O(runs).
 */
export function runParts(
  runs: FileRuns,
  clock: BlockClock,
  x0: number,
  x1: number,
  hold: boolean,
): RunPart[] {
  const out: RunPart[] = [];
  runs.forEach((run, index) => {
    let lo = xOf(clock, run.fromUs);
    let hi = xOf(clock, run.toUs);
    if (hold) {
      lo = index === 0 ? -Infinity : (xOf(clock, runs[index - 1].toUs) + lo) / 2;
      hi = index === runs.length - 1 ? Infinity : (hi + xOf(clock, runs[index + 1].fromUs)) / 2;
    }
    const xa = Math.max(x0, lo);
    const xb = Math.min(x1, hi);
    if (xb > xa) out.push({ run, index, xa, xb });
  });
  return out;
}

/** The run's own time (µs) at source time `us`. */
export const localUs = (run: FileRun, us: number) => (us - run.fromUs) * run.rate;

/** What the runs map to, as text: a paint that maps them differently (a nudge) draws again. */
const keys = new WeakMap<object, string>();
export function runsKey(runs: BlockRuns): string {
  let key = keys.get(runs);
  if (key === undefined) {
    const one = (r: FileRun) => `${r.path}@${r.fromUs}-${r.toUs}x${r.rate}+${r.inUs ?? 0}~${r.leadUs ?? 0}`;
    key = [runs.film.map(one).join(), ...runs.sounds.map((s) => s.map(one).join())].join('|');
    if (runs.gains) key += `|gains:${runs.gains.join()}`;
    if (runs.centres) key += `|centres:${runs.centres.join()}`;
    keys.set(runs, key);
  }
  return key;
}
