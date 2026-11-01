import type { Clip } from '../../../bindings/Clip';
import type { Item } from '../../../bindings/Item';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import { frameUs } from '../../../lib/time';
import { assetById, clipDurationUs } from '../../../lib/timeline';
import { fittedTransitionsUs, sourceLength } from '../../../lib/transitions';
import { mainIndex } from './index';

/** Edits that move footage across a cut or through a clip while the video keeps its length (the model's ops). */
export type EditKind = 'roll' | 'slip' | 'slide';

/** What holds an edit at one end of its room. */
export type EditLimit = 'footage' | 'length' | 'transition' | 'locked';

/**
 * How far an edit can go, in timeline µs: `min` earlier (0 or less) and `max` later (0 or more), and what holds it at
 * each end. A roll's later is the cut later; a slip's, later footage; a slide's, the clip later.
 */
export interface EditRoom {
  min: number;
  max: number;
  minBy: EditLimit;
  maxBy: EditLimit;
}

/** A separated layer's edge this near (µs) its clip's moves with it, as the model's `AT_EDGE_US`. */
const AT_EDGE_US = 1_000;
/** The shortest a layer may be (the model's `MIN_DURATION_US`). */
const MIN_DURATION_US = 10_000;

/** Rust's `f64::round`: halves away from zero (JS rounds them up). */
const round = (v: number) => Math.sign(v) * Math.round(Math.abs(v));
/** The timeline time `len` of footage plays for at `speed`, rounded down, so a move by it never reaches past it. */
const within = (len: number, speed: number) => Math.floor(Math.max(0, len) / speed);

/** The clips an edit at clip `i` changes, with whether each one's in and out points move; null when it can't act. */
function changed(kind: EditKind, main: Clip[], i: number): [number, boolean, boolean][] | null {
  if (i < 0 || i >= main.length) return null;
  if (kind === 'slip') return [[i, true, true]];
  if (kind === 'roll')
    return i + 1 < main.length
      ? [
          [i, false, true],
          [i + 1, true, false],
        ]
      : null;
  return i > 0 && i + 1 < main.length
    ? [
        [i - 1, false, true],
        [i + 1, true, false],
      ]
    : null;
}

/** `clip` with its in point (`start`) and out point (`end`) moved `by` of timeline, as the model moves them. */
function moved(clip: Clip, by: number, start: boolean, end: boolean): Clip {
  const source = round(by * clip.speed);
  return {
    ...clip,
    sourceInUs: start ? clip.sourceInUs + source : clip.sourceInUs,
    sourceOutUs: end ? clip.sourceOutUs + source : clip.sourceOutUs,
  };
}

/** A sound or video layer's file, where it plays from, and its footage per timeline µs. */
function played(project: Project, item: Item): { length: number; from: number; pace: number } | null {
  const c = item.content;
  if (c.type !== 'video' && c.type !== 'audio') return null;
  const length = assetById(project, c.assetId)?.durationUs;
  if (length === undefined) return null;
  return { length, from: c.sourceInUs, pace: c.type === 'video' ? c.speed : 1 };
}

/** Bounds as they are gathered: the tightest of each side wins, with its reason. */
class Bounds {
  min = -Infinity;
  max = Infinity;
  minBy: EditLimit = 'footage';
  maxBy: EditLimit = 'footage';
  low(v: number, by: EditLimit) {
    if (v > this.min) [this.min, this.minBy] = [v, by];
  }
  high(v: number, by: EditLimit) {
    if (v < this.max) [this.max, this.maxBy] = [v, by];
  }
}

/**
 * The room an edit of `kind` at main-track clip `i` has (a roll's at the cut after it), as the model allows it: each
 * changed clip within its media and at least a frame long, each transition keeping its length (its footage stays),
 * a recording's separated sound and camera within their files, and nothing on a locked track moved. Null when the edit
 * can't act there (a roll needs a clip after, a slide one each side). Worked out once per gesture: O(clips × log
 * range) for the transitions, O(layers) for the rest.
 */
export function editRoom(project: Project, kind: EditKind, i: number): EditRoom | null {
  const main = project.timeline.main;
  const clips = changed(kind, main, i);
  if (!clips) return null;
  const frame = frameUs(project.settings.fps);
  const b = new Bounds();
  for (const [k, start, end] of clips) {
    const clip = main[k];
    const length = clipDurationUs(clip);
    if (start) b.low(-within(clip.sourceInUs, clip.speed), 'footage');
    if (end) b.high(within(sourceLength(project, clip) - clip.sourceOutUs, clip.speed), 'footage');
    // A start that moves later shortens the clip; an end that moves earlier does too (a slip keeps its length).
    if (start && !end) b.high(length - frame, 'length');
    if (end && !start) b.low(frame - length, 'length');
  }
  separatedRoom(project, kind, i, clips, b);
  const fitted = fittedTransitionsUs(project);
  const fits = (by: number) => {
    const next = [...main];
    for (const [k, start, end] of clips) next[k] = moved(main[k], by, start, end);
    const after = fittedTransitionsUs({ ...project, timeline: { ...project.timeline, main: next } });
    return after.every((d, k) => d === fitted[k]);
  };
  // Transitions only ever hold an edit further in (each bound is linear in it): the furthest that keeps them, each way.
  const furthest = (limit: number, sign: 1 | -1) => {
    if (fits(limit)) return limit;
    let [ok, bad] = [0, limit];
    while (Math.abs(bad - ok) > 1) {
      const mid = ok + sign * Math.floor(Math.abs(bad - ok) / 2);
      if (fits(mid)) ok = mid;
      else bad = mid;
    }
    return ok;
  };
  const max = Math.max(0, b.max);
  const min = Math.min(0, b.min);
  const later = Number.isFinite(max) ? furthest(max, 1) : max;
  const earlier = Number.isFinite(min) ? furthest(min, -1) : min;
  return {
    min: earlier,
    max: later,
    minBy: earlier > min ? 'transition' : b.minBy,
    maxBy: later < max ? 'transition' : b.maxBy,
  };
}

/** A recording's separated sound and camera (`tie`) held to their files, and the layers on locked tracks it moves. */
function separatedRoom(
  project: Project,
  kind: EditKind,
  i: number,
  clips: [number, boolean, boolean][],
  b: Bounds,
) {
  const main = project.timeline.main;
  const index = mainIndex(main);
  // The clip whose footage moves on the timeline (a slip's moves in place, a slide's whole clip moves).
  const carried = kind === 'roll' ? -1 : i;
  for (const track of project.timeline.tracks) {
    for (const item of track.items) {
      const clipId = item.attach?.clipId;
      if (clipId === undefined) continue;
      const k = main.findIndex((c) => c.id === clipId);
      if (k < 0) continue;
      const tied = (item.content.type === 'video' || item.content.type === 'audio') && item.content.tie;
      if (track.locked && k === carried) return void Object.assign(b, locked());
      if (!tied) continue;
      const file = played(project, item);
      const speed = main[k].speed;
      if (kind === 'slip' && k === i) {
        if (track.locked) return void Object.assign(b, locked());
        if (!file) continue;
        const footage = round(item.durationUs * file.pace);
        b.low(-within(file.from, speed), 'footage');
        b.high(within(file.length - file.from - footage, speed), 'footage');
        continue;
      }
      const edge = clips.find(([c]) => c === k);
      if (!edge) continue;
      const [, start, end] = edge;
      const atStart = start && Math.abs(item.startUs - index.starts[k]) <= AT_EDGE_US;
      const atEnd = end && Math.abs(item.startUs + item.durationUs - index.ends[k]) <= AT_EDGE_US;
      if ((atStart || atEnd) && track.locked) return void Object.assign(b, locked());
      if (!file) continue;
      if (atStart) {
        b.low(-within(file.from, file.pace), 'footage');
        b.high(item.durationUs - MIN_DURATION_US, 'length');
      }
      if (atEnd) {
        const footage = round(item.durationUs * file.pace);
        b.high(within(file.length - file.from - footage, file.pace), 'footage');
        b.low(MIN_DURATION_US - item.durationUs, 'length');
      }
    }
  }
}

/** No room either way: a layer on a locked track would move. */
const locked = () => ({ min: 0, max: 0, minBy: 'locked' as const, maxBy: 'locked' as const });

/** `by` held to `room`, and what holds it there (null: it's within the room). */
export function heldTo(room: EditRoom, by: number): { by: number; held: EditLimit | null } {
  if (by > room.max) return { by: room.max, held: room.maxBy };
  if (by < room.min) return { by: room.min, held: room.minBy };
  return { by, held: null };
}

/** The edit as the model takes it: one op, or none for no change. */
export function editOps(kind: EditKind, clipId: string, byUs: number): Op[] {
  return byUs === 0 ? [] : [{ type: kind, clipId, byUs }];
}
