import { memo, type ReactNode } from 'react';
import type { Clip } from '../../../bindings/Clip';
import type { Track } from '../../../bindings/Track';
import { useLocale } from '../../../i18n/react';
import { mixFor, runInClip } from '../../../lib/recording';
import { paramText } from '../../../lib/sourceNames';
import { isFreeze, type Sources } from '../../../lib/timeline';
import { SOURCE_PART_MIN_PX } from '../layout';
import { BlockMedia } from '../media/BlockMedia';
import { sourceFiles } from '../media/blockFiles';
import type { BlockRuns, FileRuns } from '../media/runs';
import { mainIndex } from '../model/index';
import { mergeTiny, visibleRange } from '../model/visible';
import { usToPx } from '../scale';
import { Band } from './Band';
import { LaneRow } from './LaneRow';
import { SourceLaneHeader } from './SourceLaneHeader';
import { kindsOf, laneParam, type Kind } from './sourceKinds';

/** A part's inset in its lane: 2 px above, 3 px below. */
const PART_INSET_PX = 2 + 3;
const NO_PATHS: readonly string[] = [];

/** The files sound layers play: a source with one of them has sound on a layer of its own. Once per track list. */
const layered = new WeakMap<Track[], Set<string>>();
function layeredFiles(tracks: Track[]): Set<string> {
  let files = layered.get(tracks);
  if (!files) {
    files = new Set();
    for (const track of tracks)
      for (const item of track.items) if (item.content.type === 'audio') files.add(item.content.assetId);
    layered.set(tracks, files);
  }
  return files;
}

/**
 * A source's runs as a block's sounds (one track, no filmstrip), drawn from the channel it plays centred from or as
 * recorded (`centre`): the same object for the same runs and channel (at most three per runs).
 */
const asSound = new WeakMap<FileRuns, Map<number | null, BlockRuns>>();
function soundRuns(runs: FileRuns, centre: number | null): BlockRuns {
  let byCentre = asSound.get(runs);
  if (!byCentre) asSound.set(runs, (byCentre = new Map()));
  let out = byCentre.get(centre);
  if (!out) {
    out = centre === null ? { film: [], sounds: [runs] } : { film: [], sounds: [runs], centres: [centre] };
    byCentre.set(centre, out);
  }
  return out;
}

interface Window {
  pps: number;
  fromUs: number;
  toUs: number;
}

interface SourceLanesProps {
  /** The main track. */
  clips: Clip[];
  /** The project's files and recordings (which file carries sound, each recording's sources). */
  sources: Sources;
  tracks: Track[];
  view: Window;
  /** Each lane's height (px): smaller in a short window (`useLaneSizes`). */
  rowPx: number;
  width: number;
  onSeek: (px: number) => void;
}

/**
 * Under the Video lane, one lane per kind of recorded sound ("Mic", "Computer sound") across every recording the main
 * track plays: eight takes give one Mic lane, not eight; a recording's second mic has its own ("Mic 2"), and a camera's
 * own sound its own ("Camera sound"). Each lane's header mutes it or sets its volume everywhere, acting on its own
 * source alone, and the lane shows each recording's sound of that kind under its clips, run by run where the recorder
 * started new files, dimmed where it is muted. None shows without a recording.
 */
export function SourceLanes({ clips, sources, tracks, view, rowPx, width, onSeek }: SourceLanesProps) {
  const files = layeredFiles(tracks);
  return kindsOf(clips, sources).map((kind) => {
    const param = laneParam(kind);
    const name = paramText(param);
    const all = kind.sources.map(({ source }) => source);
    const key = kind.nth === 0 ? kind.role : `${kind.role}:${kind.nth}`;
    return (
      <LaneRow
        key={key}
        lane={`source:${key}`}
        name={name}
        header={
          <SourceLaneHeader
            role={kind.role}
            nth={kind.nth}
            sources={all}
            name={name}
            param={param}
            separated={all.some((s) => s.runs.some((run) => files.has(run.assetId)))}
          />
        }
        empty={false}
        height={rowPx}
        width={width}
        onSeek={onSeek}
      >
        <SourceLane clips={clips} kind={kind} sources={sources} innerPx={rowPx - PART_INSET_PX} {...view} />
      </LaneRow>
    );
  });
}

interface SourceLaneProps extends Window {
  clips: Clip[];
  kind: Kind;
  sources: Sources;
  innerPx: number;
}

/**
 * One kind's lane: the window's clips of a recording with that kind of sound, each with that recording's waveform of it
 * where it recorded something in that clip (`SourcePart`). Clips narrower than `SOURCE_PART_MIN_PX` join a band, so an
 * hour zoomed out mounts a few bands, not a canvas per clip. O(clips in the window).
 */
const SourceLane = memo(function SourceLane({ clips, kind, sources, innerPx, pps, fromUs, toUs }: SourceLaneProps) {
  // Re-renders on a language switch, as the other lanes do.
  useLocale();
  const index = mainIndex(clips);
  const [first, end] = visibleRange(index, fromUs, toUs);
  /** Clip `i`'s source of this kind, if it plays a recording with one (a freeze frame plays none). */
  const sourceAt = (i: number) => {
    const s = clips[i].source;
    if (s.type !== 'recording' || isFreeze(clips[i])) return undefined;
    return kind.sources.find(({ recording }) => recording.id === s.id);
  };
  const out: ReactNode[] = [];
  for (const piece of mergeTiny(index, first, end, pps, SOURCE_PART_MIN_PX, () => false)) {
    if (piece.kind === 'band') {
      let [a, z] = [Infinity, -Infinity];
      for (let i = piece.first; i <= piece.last; i++) {
        if (!sourceAt(i)) continue;
        a = Math.min(a, index.starts[i]);
        z = Math.max(z, index.ends[i]);
      }
      if (a < z)
        out.push(
          <Band key={`band:${piece.first}`} left={usToPx(a, pps)} width={usToPx(z - a, pps)} look="source" />,
        );
      continue;
    }
    const i = piece.index;
    const found = sourceAt(i);
    if (!found) continue;
    const { recording, source } = found;
    const clip = clips[i];
    if (!source.runs.some((run) => runInClip(clip, index.starts[i], source, run))) continue;
    const runs = sourceFiles(sources, recording.id, source.id);
    if (runs.length === 0) continue;
    const mix = mixFor(clip, source);
    out.push(
      <SourcePart
        key={clip.id}
        runs={soundRuns(runs, mix.centreFrom ?? null)}
        startUs={index.starts[i]}
        durationUs={index.ends[i] - index.starts[i]}
        sourceInUs={clip.sourceInUs}
        speed={clip.speed}
        volume={clip.volume * mix.volume}
        fadeInUs={clip.fadeInUs}
        fadeOutUs={clip.fadeOutUs}
        silent={clip.muted}
        muted={mix.muted}
        pps={pps}
        height={innerPx}
      />,
    );
  }
  return out;
});

interface SourcePartProps {
  runs: BlockRuns;
  startUs: number;
  durationUs: number;
  sourceInUs: number;
  speed: number;
  volume: number;
  fadeInUs: number;
  fadeOutUs: number;
  /** The clip itself is muted: a flat line. */
  silent: boolean;
  /** The source is muted in this clip (its own mix, or the default): its shape stays, dimmed. */
  muted: boolean;
  pps: number;
  height: number;
}

/**
 * One clip's part of a source lane: the source's waveform over the clip, run by run, in the clip's blue. Nothing to
 * select: the clip on the Video lane is what a press picks. Memoized on primitives.
 */
const SourcePart = memo(function SourcePart({ runs, startUs, durationUs, pps, muted, silent, ...how }: SourcePartProps) {
  const left = usToPx(startUs, pps);
  return (
    <span
      aria-hidden
      data-source-part
      data-muted={muted || undefined}
      className="tl-source-part"
      style={{ left, width: Math.max(1, usToPx(durationUs, pps)) }}
    >
      <BlockMedia
        kind="audio"
        peakPaths={NO_PATHS}
        runs={runs}
        startUs={startUs}
        durationUs={durationUs}
        muted={silent}
        faded={muted}
        pps={pps}
        waveToken="--color-clip-2"
        {...how}
      />
    </span>
  );
});
