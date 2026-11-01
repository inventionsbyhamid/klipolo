import { memo } from 'react';
import type { Item } from '../../bindings/Item';
import type { Track } from '../../bindings/Track';
import { cn } from '../../design/cn';
import { useProposal, type Proposal } from '../../state/proposal';
import { audioBox } from './lanes/AudioLane';
import { audioLines, audioRole, type AudioRole } from './lanes/audioRole';
import { packedBox } from './lanes/LayersLane';
import { rowBox, type Box } from './lanes/TrackLane';
import { packRows } from './model/packRows';
import { usToPx } from './scale';

interface Window {
  pps: number;
  fromUs: number;
  toUs: number;
}

/**
 * Which lane the overlay covers: the Overlays lane, or the Sounds or Music lane (`rowPx` tall); or, while every track
 * shows, one track's row (`track`, `rowPx` tall).
 */
export type ProposalOverlayProps = Window &
  ({ lane: 'layers' | AudioRole; tracks: Track[]; rowPx?: number } | { lane: 'track'; track: Track; rowPx: number });

/**
 * What goes on one lane, in start order, with the furthest end reached so far (`reach`), so the window's are found by
 * binary search however many there are: `[starts[i], ends[i])` drawn in `boxes[i]` (null: the lane's full height),
 * dashed when the layer only gets shorter.
 */
interface Ghosts {
  starts: Float64Array;
  ends: Float64Array;
  reach: Float64Array;
  boxes: (Box | null)[];
  shortened: Uint8Array;
}

interface Ghost {
  start: number;
  end: number;
  box: Box | null;
  shortened: boolean;
}

function index(list: Ghost[]): Ghosts {
  list.sort((a, b) => a.start - b.start);
  const n = list.length;
  const ghosts: Ghosts = {
    starts: new Float64Array(n),
    ends: new Float64Array(n),
    reach: new Float64Array(n),
    boxes: new Array<Box | null>(n),
    shortened: new Uint8Array(n),
  };
  let reach = -Infinity;
  list.forEach((g, i) => {
    ghosts.starts[i] = g.start;
    ghosts.ends[i] = g.end;
    reach = Math.max(reach, g.end);
    ghosts.reach[i] = reach;
    ghosts.boxes[i] = g.box;
    ghosts.shortened[i] = g.shortened ? 1 : 0;
  });
  return ghosts;
}

/** The proposal's removed and shortened layers, by id (true: shortened). Built once per proposal. */
const affected = new WeakMap<Proposal, Map<string, boolean>>();
function affectedBy(proposal: Proposal): Map<string, boolean> {
  let found = affected.get(proposal);
  if (!found) {
    found = new Map(proposal.removals.items.map((id) => [id, false]));
    for (const id of proposal.removals.shortened) if (!found.has(id)) found.set(id, true);
    affected.set(proposal, found);
  }
  return found;
}

/** A lane's ghosts: the main lane's, the Overlays lane's, or an audio lane's at its height (`sounds:22`). */
type LaneKind = string;

/**
 * Each lane's ghosts, built once per proposal and per lane content (a patch keeps an unchanged track's arrays),
 * so scrolling and zooming only search them.
 */
const built = new WeakMap<Proposal, Map<LaneKind, WeakMap<object, Ghosts>>>();
function ghostsFor(proposal: Proposal, lane: LaneKind, key: object, build: () => Ghost[]): Ghosts {
  let lanes = built.get(proposal);
  if (!lanes) built.set(proposal, (lanes = new Map()));
  let byKey = lanes.get(lane);
  if (!byKey) lanes.set(lane, (byKey = new WeakMap()));
  let ghosts = byKey.get(key);
  if (!ghosts) {
    ghosts = index(build());
    byKey.set(key, ghosts);
  }
  return ghosts;
}

function layerGhosts(items: Item[], gone: Map<string, boolean>, box: (id: string) => Box): Ghost[] {
  const out: Ghost[] = [];
  for (const item of items) {
    const shortened = gone.get(item.id);
    if (shortened === undefined) continue;
    out.push({ start: item.startUs, end: item.startUs + item.durationUs, box: box(item.id), shortened });
  }
  return out;
}

/** The footage that goes, on the timeline as it is now: one range each (`ProposalRanges` draws them). */
export function mainGhosts(proposal: Proposal): Ghosts {
  const { ranges } = proposal.removals;
  return ghostsFor(proposal, 'main', ranges, () =>
    ranges.map((r) => ({ start: r.start, end: r.end, box: null, shortened: false })),
  );
}

/** The Sounds or the Music lane's sound layers that go or get shorter, in a lane `rowPx` tall. */
function audioGhosts(proposal: Proposal, tracks: Track[], role: AudioRole, rowPx: number): Ghosts {
  const box = audioBox(role, rowPx);
  const gone = affectedBy(proposal);
  return ghostsFor(proposal, `${role}:${rowPx}`, tracks, () =>
    audioLines(tracks)[role].lines.flatMap((line) => layerGhosts(line.items, gone, () => box)),
  );
}

/** Compact mode's layers: each in the packed row it draws in; one packed away, in the last row (where its "+N" is). */
function layersGhosts(proposal: Proposal, tracks: Track[]): Ghosts {
  const gone = affectedBy(proposal);
  return ghostsFor(proposal, 'layers', tracks, () => {
    const packing = packRows(tracks);
    const last = packedBox(Math.max(0, packing.rows - 1));
    return tracks.flatMap((t) =>
      t.kind === 'audio'
        ? []
        : layerGhosts(t.items, gone, (id) => {
            const row = packing.row.get(id);
            return row === undefined ? last : packedBox(row);
          }),
    );
  });
}

/** One track's layers that go or get shorter, in its row `rowPx` tall, where its row draws each. */
function trackGhosts(proposal: Proposal, track: Track, rowPx: number): Ghosts {
  const gone = affectedBy(proposal);
  return ghostsFor(proposal, `track:${rowPx}`, track, () => {
    const boxOf = (id: string) => {
      const item = track.items.find((i) => i.id === id);
      return track.kind === 'audio' && item ? audioBox(audioRole(item), rowPx) : rowBox(rowPx);
    };
    return layerGhosts(track.items, gone, boxOf);
  });
}

function laneGhosts(proposal: Proposal, props: ProposalOverlayProps): Ghosts {
  if (props.lane === 'track') return trackGhosts(proposal, props.track, props.rowPx);
  return props.lane === 'layers'
    ? layersGhosts(proposal, props.tracks)
    : audioGhosts(proposal, props.tracks, props.lane, props.rowPx ?? 0);
}

/** The window's part of `ghosts`: the index of each that reaches into [fromUs, toUs), in start order. */
export function inWindow(ghosts: Ghosts, fromUs: number, toUs: number): number[] {
  const { starts, ends, reach } = ghosts;
  // The first whose run reaches into the window; from there, those starting before its end.
  let lo = 0;
  let hi = reach.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (reach[mid] <= fromUs) lo = mid + 1;
    else hi = mid;
  }
  const shown: number[] = [];
  for (let i = lo; i < starts.length && starts[i] < toUs; i++) if (ends[i] > fromUs) shown.push(i);
  return shown;
}

/**
 * While a proposal waits for the person, the layers it removes show on their lanes as a red outline (dashed when one
 * only gets shorter); the footage it cuts shows across every lane (`ProposalRanges`). Times are the timeline's before
 * the change, which is what is on screen until Apply. Only the window's are drawn: a binary search, then a walk over
 * what overlaps it. Clicks pass through.
 */
export const ProposalOverlay = memo(function ProposalOverlay(props: ProposalOverlayProps) {
  const proposal = useProposal((s) => s.proposal);
  if (!proposal) return null;
  const ghosts = laneGhosts(proposal, props);
  const { pps, fromUs, toUs } = props;
  const { starts, ends, boxes, shortened } = ghosts;
  const shown = inWindow(ghosts, fromUs, toUs).map((i) => {
    // Clipped to the window, so a long removal zoomed in is never a huge element.
    const from = Math.max(starts[i], fromUs);
    const to = Math.min(ends[i], toUs);
    const left = usToPx(from, pps);
    const box = boxes[i]!;
    return (
      <div
        key={i}
        data-ghost={shortened[i] ? 'shortened' : 'layer'}
        className={cn('tl-ghost', shortened[i] === 1 && 'is-shortened')}
        style={{ left, width: Math.max(2, usToPx(to, pps) - left), top: box.top, height: box.height }}
      />
    );
  });
  if (shown.length === 0) return null;
  return (
    <div aria-hidden className="pointer-events-none absolute inset-0 z-[6]">
      {shown}
    </div>
  );
});
