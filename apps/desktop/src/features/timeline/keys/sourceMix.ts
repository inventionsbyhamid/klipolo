import type { Clip } from '../../../bindings/Clip';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import type { Recording } from '../../../bindings/Recording';
import type { RecordingSource } from '../../../bindings/RecordingSource';
import type { SourceMix } from '../../../bindings/SourceMix';
import type { SourceRole } from '../../../bindings/SourceRole';
import { deepEqual } from '../../../lib/equal';
import { isVideoRole, makesSound, mixFor } from '../../../lib/recording';
import { itemEndUs } from '../../../lib/timeline';
import { mainIndex } from '../model/index';

/*
 * A kind of recorded sound ("Mic", "Computer sound", "Camera sound") across every recording: what a source lane's
 * header mutes, sets the volume of or centres. A lane is one place among a kind's sources: lane `nth` acts on each
 * recording's `nth` source of the kind, and a picture role's lane on the sound its files carry.
 */

/** What a lane's control makes of a source's mix (given the source too, for a change that depends on it). */
export type MixChange = (mix: SourceMix, source: RecordingSource) => SourceMix;

/** Recording `r`'s `nth` source of `role` (0 is the first). O(sources). */
export function sourceOfKind(r: Recording, role: SourceRole, nth = 0): RecordingSource | undefined {
  let place = 0;
  for (const s of r.sources) if (s.role === role && place++ === nth) return s;
  return undefined;
}

/**
 * Where a source plays from layers of its own (separated): each sound or video layer of its files, sorted by start,
 * each with the furthest end reached so far. O(layers + own layers × log).
 */
export class OwnTrack {
  private constructor(private readonly spans: readonly (readonly [number, number])[]) {}

  static of(project: Project, source: RecordingSource): OwnTrack {
    const files = new Set(source.runs.map((run) => run.assetId));
    const spans: [number, number][] = [];
    for (const track of project.timeline.tracks)
      for (const item of track.items) {
        const c = item.content;
        if ((c.type === 'audio' || c.type === 'video') && files.has(c.assetId))
          spans.push([item.startUs, itemEndUs(item)]);
      }
    spans.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
    for (let k = 1; k < spans.length; k++) spans[k][1] = Math.max(spans[k][1], spans[k - 1][1]);
    return new OwnTrack(spans);
  }

  /** Some of it plays from a layer of its own. */
  any(): boolean {
    return this.spans.length > 0;
  }

  /** One of its layers plays somewhere in [start, end): a binary search. */
  over(start: number, end: number): boolean {
    let lo = 0;
    let hi = this.spans.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.spans[mid][0] < end) lo = mid + 1;
      else hi = mid;
    }
    return lo > 0 && this.spans[lo - 1][1] > start;
  }
}

/** Whether main clip `i`'s sound of `source` plays from a layer of its own (so its mix there is left as it is). */
export function onOwnTrack(project: Project, i: number, source: RecordingSource): boolean {
  if (i < 0 || i >= project.timeline.main.length) return false;
  const { starts, ends } = mainIndex(project.timeline.main);
  return OwnTrack.of(project, source).over(starts[i], ends[i]);
}

/**
 * Every recording's `nth` source of `role` changed by `change`, everywhere: its default, and each clip of its recording
 * with a mix of its own, from what that clip plays now; a clip's mix that ends up the default is dropped (`null`), and a
 * clip whose sound plays from its own track keeps its mix. A picture source counts only where its files carry sound (a
 * take's camera has none). Recordings in the project's order, each its default first unless it stays as it is while
 * clips of their own change. O(recordings × (clips + layers)).
 */
export function kindMixOps(project: Project, role: SourceRole, change: MixChange, nth = 0): Op[] {
  const main = project.timeline.main;
  const { starts, ends } = mainIndex(main);
  const ops: Op[] = [];
  for (const r of project.recordings) {
    const source = sourceOfKind(r, role, nth);
    if (!source || !makesSound(project, source)) continue;
    const layers = OwnTrack.of(project, source);
    const def = change(source.mix, source);
    const first = ops.length;
    const own = (c: Clip) =>
      c.source.type === 'recording' && c.source.id === r.id && c.sourceMix?.[source.id] !== undefined;
    main.forEach((clip, i) => {
      if (!own(clip) || layers.over(starts[i], ends[i])) return;
      const next = change(mixFor(clip, source), source);
      const mix = deepEqual(next, def) ? null : next;
      if (!deepEqual(clip.sourceMix?.[source.id] ?? null, mix))
        ops.push({ type: 'setClipSourceMix', clipId: clip.id, sourceId: source.id, mix });
    });
    if (!deepEqual(def, source.mix) || ops.length === first)
      ops.splice(first, 0, {
        type: 'updateRecordingSource',
        recordingId: r.id,
        sourceId: source.id,
        patch: { mix: def },
      });
  }
  return ops;
}

/**
 * `mix` with `fillsGaps` as `on`, left out while off as Rust writes it. A mix the person muted stays silent: the
 * person's mute always wins.
 */
function filling(mix: SourceMix, on: boolean): SourceMix {
  const next: SourceMix = { ...mix, fillsGaps: true };
  if (!on || personMuted(mix)) delete next.fillsGaps;
  return next;
}

/** Whether the person muted `mix` (Rust's `SourceMix::person_muted`): no fill turns it back on. */
export const personMuted = (mix: SourceMix) => mix.muted && mix.mutedByPerson === true;

/**
 * Whether `mix` plays nothing: muted, and not filling the gaps. A camera's sound kept muted while it fills the gaps
 * still plays there, so a clip's mute toggle mutes it for real first.
 */
export const silent = (mix: SourceMix) => mix.muted && mix.fillsGaps !== true;

/**
 * Recording `r`'s camera sound fills the gaps in its recorded sound (`on`) or leaves them silent. Each picture source
 * whose files carry sound: its default where it changes, then each clip of `r` with a mix of its own of it that
 * changes, dropped where it becomes the default (no ops at all: it is already so). O(sources × clips).
 */
export function fillGapsOps(project: Project, r: Recording, on: boolean): Op[] {
  const ops: Op[] = [];
  for (const s of r.sources) {
    if (!isVideoRole(s.role) || !makesSound(project, s)) continue;
    const def = filling(s.mix, on);
    // A default that stays as it is makes no op.
    if (!deepEqual(def, s.mix))
      ops.push({ type: 'updateRecordingSource', recordingId: r.id, sourceId: s.id, patch: { mix: def } });
    for (const clip of project.timeline.main) {
      const own =
        clip.source.type === 'recording' && clip.source.id === r.id ? clip.sourceMix?.[s.id] : undefined;
      if (!own) continue;
      const mix = filling(own, on);
      const same = deepEqual(mix, def);
      if (!deepEqual(mix, own) || same)
        ops.push({ type: 'setClipSourceMix', clipId: clip.id, sourceId: s.id, mix: same ? null : mix });
    }
  }
  return ops;
}

/**
 * `mix` muted or unmuted. A mute means silence: muting a camera's sound also stops it filling the gaps, and marks the
 * mute the person's (Rust's `SourceMix::set_muted`); unmuting clears the mark and leaves the fill as it is. Fields left
 * out while false, as Rust writes them.
 */
export function mutedMix(mix: SourceMix, muted: boolean): SourceMix {
  const next: SourceMix = { ...mix, muted, mutedByPerson: true };
  if (muted) delete next.fillsGaps;
  else delete next.mutedByPerson;
  return next;
}

/** Mute or unmute a kind of sound everywhere. */
export const kindMuteOps = (project: Project, role: SourceRole, muted: boolean, nth = 0) =>
  kindMixOps(project, role, (mix) => mutedMix(mix, muted), nth);

/** A kind of sound's volume everywhere (0 to 2, to two decimals, as a percent is). */
export const kindVolumeOps = (project: Project, role: SourceRole, volume: number, nth = 0) => {
  const v = Math.round(volume * 100) / 100;
  return kindMixOps(project, role, (mix) => ({ ...mix, volume: v }), nth);
};

/** How loud each recorded file is on each of its channels (0 to 1), by asset id. */
export type ChannelPeaks = ReadonlyMap<string, readonly number[]>;

/**
 * The channel `source` plays from when centred: the side that carries its sound. Where `peaks` knows its files, the
 * louder of the two over its two-channel runs (a one-channel run plays centred either way; a tie goes to the first);
 * else the side it already plays centred from; else the first. O(runs).
 */
export function liveChannel(source: RecordingSource, peaks: ChannelPeaks): number {
  let loudest: [number, number] | null = null;
  for (const run of source.runs) {
    const p = peaks.get(run.assetId);
    if (!p || p.length < 2) continue;
    const [l, r]: [number, number] = loudest ?? [0, 0];
    loudest = [Math.max(l, p[0]), Math.max(r, p[1])];
  }
  return loudest ? Number(loudest[1] > loudest[0]) : (source.mix.centreFrom ?? 0);
}

/** `mix` played centred from `channel`, or as recorded (null): the field left out then, as Rust leaves it out. */
function centredMix(mix: SourceMix, channel: number | null): SourceMix {
  const out = { ...mix };
  if (channel === null) delete out.centreFrom;
  else out.centreFrom = channel;
  return out;
}

/** A kind of sound everywhere in both ears from the side that carries it, or as recorded. */
export const kindCentreOps = (
  project: Project,
  role: SourceRole,
  centred: boolean,
  peaks: ChannelPeaks,
  nth = 0,
) =>
  kindMixOps(
    project,
    role,
    (mix, source) => centredMix(mix, centred ? liveChannel(source, peaks) : null),
    nth,
  );

/** Whether `ops` change nothing (the default as it was, no clip touched). */
export function changesNothing(project: Project, ops: Op[]): boolean {
  return ops.every((op) => {
    if (op.type !== 'updateRecordingSource') return false;
    const s = project.recordings
      .find((r) => r.id === op.recordingId)
      ?.sources.find((x) => x.id === op.sourceId);
    return s !== undefined && deepEqual(op.patch.mix, s.mix);
  });
}
