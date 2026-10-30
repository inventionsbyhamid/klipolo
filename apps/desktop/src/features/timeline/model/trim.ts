import type { Clip } from '../../../bindings/Clip';
import type { Edge } from '../../../bindings/Edge';
import type { Item } from '../../../bindings/Item';
import type { Project } from '../../../bindings/Project';
import type { Track } from '../../../bindings/Track';
import { frameUs } from '../../../lib/time';
import { assetById, clipDurationUs } from '../../../lib/timeline';
import { sourceLength } from '../../../lib/transitions';

// `|| 0` turns −0 (a clamp at −0) into 0, so callers and tests never see a negative zero.
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v)) || 0;
/** The timeline time `len` of footage plays for, rounded down: at 2×, a trim to it never reaches past the file. */
const within = (len: number, speed: number) => Math.floor(len / speed);

/** A main clip's `edge` moved by `deltaUs` of timeline, held to its media and to one frame (the model ripples). */
export function trimClip(project: Project, clip: Clip, edge: Edge, deltaUs: number, fps: number) {
  const frame = frameUs(fps);
  const length = clipDurationUs(clip);
  if (edge === 'start') {
    const d = clamp(deltaUs, -within(clip.sourceInUs, clip.speed), length - frame);
    return { sourceUs: clip.sourceInUs + Math.round(d * clip.speed), deltaUs: d };
  }
  const d = clamp(
    deltaUs,
    frame - length,
    within(sourceLength(project, clip) - clip.sourceOutUs, clip.speed),
  );
  return { sourceUs: clip.sourceOutUs + Math.round(d * clip.speed), deltaUs: d };
}

/** Where a layer's `edge` may go near `atUs`: within its media (video and sound), one frame long, and never over its
 * neighbours on the track (nothing jumps tracks). */
export function trimItemEdge(
  project: Project,
  track: Track,
  item: Item,
  edge: Edge,
  atUs: number,
  fps: number,
): number {
  const frame = frameUs(fps);
  const i = track.items.findIndex((x) => x.id === item.id);
  const prevEnd = i > 0 ? track.items[i - 1].startUs + track.items[i - 1].durationUs : 0;
  const nextStart = i >= 0 && i + 1 < track.items.length ? track.items[i + 1].startUs : Infinity;
  const end = item.startUs + item.durationUs;
  const c = item.content;
  const media =
    c.type === 'video' || c.type === 'audio' ? assetById(project, c.assetId)?.durationUs : undefined;
  const speed = c.type === 'video' ? c.speed : 1;
  const sourceIn = c.type === 'video' || c.type === 'audio' ? c.sourceInUs : 0;
  if (edge === 'start') {
    const earliest = media !== undefined ? item.startUs - within(sourceIn, speed) : 0;
    return clamp(atUs, Math.max(prevEnd, earliest, 0), end - frame);
  }
  const latest = media !== undefined ? item.startUs + within(media - sourceIn, speed) : Infinity;
  return clamp(atUs, item.startUs + frame, Math.min(nextStart, latest));
}
