import type { AnalysisStep } from '../../bindings/AnalysisStep';
import type { AssetState } from '../../bindings/AssetState';
import type { Estimate } from '../../bindings/Estimate';

/** The steps in order, as the step dots show them. */
export const STEPS: AnalysisStep[] = ['audio', 'transcript', 'words', 'scenes'];

/** Each step in the person's words. */
export const STEP_LABELS: Record<AnalysisStep, string> = {
  audio: 'Getting audio',
  transcript: 'Transcribing',
  words: 'Timing words',
  scenes: 'Watching the footage',
};

/** What a clip whose file was moved or deleted needs, as Rust says it when such a clip is started (`FILE_MISSING`). */
export const FILE_MISSING = "Klipolo can't find this clip's file. Put it back or import it again.";

/** "1 clip", "2 clips". */
export function clipCount(n: number): string {
  return n === 1 ? '1 clip' : `${n} clips`;
}

/** Dollars and cents: "$0.04". */
export function dollars(usd: number): string {
  return `$${Math.max(0, usd).toFixed(2)}`;
}

/** What it costs, as the offer says it: "about $0.04", or "less than $0.01" for a sliver. */
export function about(usd: number): string {
  return usd < 0.01 ? 'less than $0.01' : `about ${dollars(usd)}`;
}

/** The offer's title: "Analyze 2 clips for about $0.04". */
export function offerTitle(estimate: Estimate): string {
  return `Analyze ${clipCount(estimate.clips.length)} for ${about(estimate.usd)}`;
}

/**
 * How long it takes, roughly: word timing runs at about the speed of the sound, and the rest overlaps it, so about as
 * long as the clips.
 */
export function howLong(seconds: number): string {
  const minutes = Math.max(1, Math.round(seconds / 60));
  return minutes === 1 ? 'Takes about a minute.' : `Takes about ${minutes} minutes.`;
}

/**
 * The offer's sentence: what Analyze does and how long it takes, what clips analyzed before cost, and with an access
 * code (or the free credit: `trial`) the credit left, or why Analyze is refused (Rust's sentence, which suggests fewer
 * or shorter files: analysis covers each file whole, however its clip is trimmed).
 */
export function offerText(estimate: Estimate, videos: boolean, trial = false): string {
  const them = estimate.clips.length === 1 ? 'it' : 'them';
  const parts = [
    videos
      ? `Klipolo transcribes ${them}, times each word and watches the footage.`
      : `Klipolo transcribes ${them} and times each word.`,
    howLong(estimate.seconds),
  ];
  const cached = estimate.cached.length;
  if (cached > 0) parts.push(`${clipCount(cached)} analyzed before ${cached === 1 ? 'loads' : 'load'} at no cost.`);
  if (estimate.refusal) parts.push(estimate.refusal);
  else if (estimate.creditLeftUsd != null && trial)
    parts.push(`Free credit: ${dollars(estimate.creditLeftUsd)} left. No key needed.`);
  else if (estimate.creditLeftUsd != null)
    parts.push(`You have ${dollars(estimate.creditLeftUsd)} left on your access code.`);
  return parts.join(' ');
}

/** `ids` as the clips the person counts, in order: a take's files (`takes`, from `useTakes`) are its one clip. */
export function byTake(ids: string[], takes: ReadonlyMap<string, string>): string[] {
  return [...new Set(ids.map((id) => takes.get(id) ?? id))];
}

/** `estimate` as the offer counts it: a take's files are one clip (`byTake`). What is sent stays by file. */
export function counted(estimate: Estimate, takes: ReadonlyMap<string, string>): Estimate {
  return { ...estimate, clips: byTake(estimate.clips, takes), cached: byTake(estimate.cached, takes) };
}

const NOT_ANALYSED: AssetState = { state: 'none' };

/**
 * A take's state, as its one row shows it, from its files': running while any runs (the least far along, so the row
 * never runs ahead), else failed while any failed; else not analysed. One of `states` itself, so a store selector can
 * return it.
 */
export function takeState(states: AssetState[]): AssetState {
  let shown = NOT_ANALYSED;
  for (const s of states) {
    if (s.state === 'running') {
      const behind =
        shown.state !== 'running' ||
        STEPS.indexOf(s.step) < STEPS.indexOf(shown.step) ||
        (s.step === shown.step && s.pct < shown.pct);
      if (behind) shown = s;
    } else if (s.state === 'failed' && shown.state === 'none') {
      shown = s;
    }
  }
  return shown;
}

/** The title over clips whose file is gone, when nothing else is offered: "Klipolo can't find 2 clips". */
export function missingTitle(n: number): string {
  return `Klipolo can't find ${clipCount(n)}`;
}

/** The progress banner's title. */
export function progressTitle(running: number, failed: number): string {
  if (running > 0) return `Analyzing ${clipCount(running)}`;
  return `Couldn't finish ${clipCount(failed)}`;
}

/** One step dot: done, the one running (filled to `pct`), or still to come. */
export type Dot = { kind: 'done' } | { kind: 'now'; pct: number } | { kind: 'next' };

/** The four step dots for a clip at `step`, `pct` of the way through it. */
export function stepDots(step: AnalysisStep, pct: number): Dot[] {
  const at = STEPS.indexOf(step);
  return STEPS.map((_, i) =>
    i < at ? { kind: 'done' } : i === at ? { kind: 'now', pct: Math.min(100, Math.max(0, pct)) } : { kind: 'next' },
  );
}

/** What a screen reader hears of a clip's progress: every 25%, not every percent (live regions). */
export function spokenProgress(name: string, step: AnalysisStep, pct: number): string {
  return `${name}: ${STEP_LABELS[step]}, ${Math.floor(Math.min(100, Math.max(0, pct)) / 25) * 25}%`;
}
