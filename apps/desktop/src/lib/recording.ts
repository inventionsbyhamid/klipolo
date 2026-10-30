import type { Asset } from '../bindings/Asset';
import type { Clip } from '../bindings/Clip';
import type { Project } from '../bindings/Project';
import type { Recording } from '../bindings/Recording';
import type { RecordingSource } from '../bindings/RecordingSource';
import type { SourceMix } from '../bindings/SourceMix';
import type { SourceRole } from '../bindings/SourceRole';
import type { SourceRun } from '../bindings/SourceRun';

/** A recording's sources by role (format 4), as `klipolo_model::recording` reads them. */

const VIDEO: ReadonlySet<SourceRole> = new Set(['screen', 'game', 'camera']);
export const isVideoRole = (role: SourceRole) => VIDEO.has(role);
export const primarySource = (r: Recording) => r.sources.find((s) => s.id === r.primary);

/** The main footage when it is a screen or a game. */
export function screenSource(r: Recording): RecordingSource | undefined {
  const p = primarySource(r);
  return p && p.role !== 'camera' ? p : undefined;
}

/** The first camera that isn't the main footage: the bubble. */
export const cameraSource = (r: Recording) =>
  r.sources.find((s) => s.role === 'camera' && s.id !== r.primary);
export const audioSources = (r: Recording) => r.sources.filter((s) => !isVideoRole(s.role));
export const firstAsset = (s: RecordingSource | undefined) => s?.runs[0]?.assetId ?? null;

/** The ids of the files that carry sound, once per asset list (which keeps its identity across edits that leave it). */
const sounding = new WeakMap<Asset[], Set<string>>();
function soundingFiles(assets: Asset[]): Set<string> {
  let ids = sounding.get(assets);
  if (!ids) sounding.set(assets, (ids = new Set(assets.filter((a) => a.hasAudio).map((a) => a.id))));
  return ids;
}

/**
 * Whether a recording clip can be heard from `s`: a sound role, or a picture source whose files carry sound (an
 * imported camera's own sound; Klipolo's picture files have none). As `query::makes_sound`. O(runs).
 */
export function makesSound(project: Pick<Project, 'assets'>, s: RecordingSource): boolean {
  if (!isVideoRole(s.role)) return true;
  const files = soundingFiles(project.assets);
  return s.runs.some((run) => files.has(run.assetId));
}

/**
 * The sources a recording clip can be heard from, in source order: every sound role, and a picture source whose files
 * carry sound (a synced camera's own sound). Mirrors `query::sound_sources`, pinned by `recording-parity.json`; the
 * source lanes and the Sounds popover list these.
 */
export const soundSources = (project: Pick<Project, 'assets'>, r: Recording): RecordingSource[] =>
  r.sources.filter((s) => makesSound(project, s));

/** `s`'s place among its recording's sources of its role (0 is the first): chat's `which` is this plus one. */
export const nthOfRole = (r: Recording, s: RecordingSource): number =>
  r.sources.filter((x) => x.role === s.role).indexOf(s);
export const cameraAssets = (r: Recording) => cameraSource(r)?.runs.map((run) => run.assetId) ?? [];

/** How `source` plays in `clip`: the clip's own mix of it, else the source's default (`Clip::mix_for`). */
export const mixFor = (clip: Pick<Clip, 'sourceMix'>, source: RecordingSource): SourceMix =>
  clip.sourceMix?.[source.id] ?? source.mix;

/** Rust's `f64::round`: halves away from zero (`Math.round` takes −2.5 to −2). */
const round = (x: number) => Math.sign(x) * Math.round(Math.abs(x));

/** The file's own µs per recording µs: its device clock ran `driftPpm` fast (`SourceRun::rate`). */
export const runRate = (run: SourceRun) => 1 + run.driftPpm * 1e-6;

/**
 * A sound layer's file µs per timeline µs: a recording source separated onto its own track keeps its run's drift, and
 * the engine plays it at that rate (`audio::plan`); 1 for any other sound.
 */
export const layerRate = (content: { driftPpm?: number }) => 1 + (content.driftPpm ?? 0) * 1e-6;

/** [from, to) on the recording clock after the nudge; the duration shrinks by the drift (`SourceRun::rec_span`). */
export function runSpan(run: SourceRun, nudgeUs: number): [number, number] {
  const from = run.offsetUs + nudgeUs;
  return [from, from + round(run.durationUs / runRate(run))];
}

/**
 * The run's own time at recording time `us`: (us − nudge − offset) × its rate, rounded, plus where in its file the run
 * starts (a synced sound that started before the video; `SourceRun::local_us`).
 */
export const runLocalUs = (run: SourceRun, us: number, nudgeUs: number) =>
  round((us - nudgeUs - run.offsetUs) * runRate(run)) + (run.fileInUs ?? 0);

/** How long the take is: the latest end of any source, as `Recording::duration_us` (a run's length shrinks by its
 * drift). The source length of a recording clip. */
export function recordingDurationUs(r: Recording): number {
  let end = 0;
  for (const s of r.sources) for (const run of s.runs) end = Math.max(end, runSpan(run, s.nudgeUs)[1]);
  return end;
}

/** The run playing at recording time `us` and its own time there, as `RecordingSource::run_at` (drift and nudge
 * included; pinned by `recording-parity.json`). Null in a gap. */
export function runAt(s: RecordingSource, us: number): { run: SourceRun; localUs: number } | null {
  for (const run of s.runs) {
    const [from, to] = runSpan(run, s.nudgeUs);
    if (us >= from && us < to) return { run, localUs: runLocalUs(run, us, s.nudgeUs) };
  }
  return null;
}

/** Where a run plays inside a clip: its timeline span, and the run's own time at the span's start. */
export interface RunInClip {
  startUs: number;
  endUs: number;
  runInUs: number;
}

/**
 * Where `run` of `source` plays inside `clip`, which starts at `clipStartUs` on the timeline; null when the run doesn't
 * overlap the clip's footage. Mirrors `query::run_in_clip` (pinned by `recording-parity.json`).
 */
export function runInClip(
  clip: Pick<Clip, 'sourceInUs' | 'sourceOutUs' | 'speed'>,
  clipStartUs: number,
  source: RecordingSource,
  run: SourceRun,
): RunInClip | null {
  const [runFrom, runTo] = runSpan(run, source.nudgeUs);
  const from = Math.max(runFrom, clip.sourceInUs);
  const to = Math.min(runTo, clip.sourceOutUs);
  if (from >= to) return null;
  return {
    startUs: clipStartUs + round((from - clip.sourceInUs) / clip.speed),
    endUs: clipStartUs + round((to - clip.sourceInUs) / clip.speed),
    runInUs: runLocalUs(run, from, source.nudgeUs),
  };
}
