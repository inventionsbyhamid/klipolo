import { memo, type ReactNode } from 'react';
import type { Asset } from '../../../bindings/Asset';
import type { Clip } from '../../../bindings/Clip';
import type { Recording } from '../../../bindings/Recording';
import { useLocale } from '../../../i18n/react';
import { isInstalled } from '../../../lib/contrib';
import { sourceLabel } from '../../../lib/timeline';
import { defaultTransitionUs, splitDuration, type Footage } from '../../../lib/transitions';
import { useProject } from '../../../state/project';
import { transitionClipId } from '../../../state/selection';
import { ClipBlock } from '../blocks/ClipBlock';
import { CutMarker } from '../blocks/CutMarker';
import { CutTip } from '../blocks/CutTip';
import { useDragged } from '../drag/controller';
import type { PressBlock, PressTrim } from '../drag/useBlockDrag';
import { cutClipId, cutId } from '../keys/laneOrder';
import {
  addTransition,
  enterCut,
  fadesReplaced,
  pickTransition,
  transitionKind,
  transitionName,
} from '../keys/transitionActions';
import { CHIP_MIN_PX, CLIP_INNER_PX, CLIP_MIN_PX, CUT_MIN_PX } from '../layout';
import { clipMediaFiles } from '../media/blockFiles';
import { mainIndex, type MainIndex } from '../model/index';
import { mergeTiny, visibleRange, withKept } from '../model/visible';
import { pxToUs, usToPx } from '../scale';
import { Band } from './Band';

/** A bare cut without room: Enter or a click says why (`enterCut` toasts it). */
function noRoomAt(clipId: string) {
  const project = useProject.getState().snapshot?.project;
  if (project) enterCut(project, clipId);
}

/** Each clip's index by id, built once per main-track array (for the few kept cuts outside the window). */
const indexes = new WeakMap<Clip[], Map<string, number>>();
function clipIndexOf(main: Clip[], id: string): number {
  let map = indexes.get(main);
  if (!map) {
    map = new Map(main.map((clip, i) => [clip.id, i]));
    indexes.set(main, map);
  }
  return map.get(id) ?? -1;
}

/**
 * The cuts whose markers draw (the clip before each, by index), for clips [first, end) and the cut before them: a
 * transition's chip between clips at least `CHIP_MIN_PX` wide, a bare cut's Add transition or note between clips at
 * least `CUT_MIN_PX` wide (zoomed out they would cover the clips, and an hour's cuts would mount hundreds of them),
 * plus a focused cut's or a selected transition's anywhere.
 */
function drawnCuts(
  main: Clip[],
  index: MainIndex,
  first: number,
  end: number,
  pps: number,
  keep: ReadonlySet<string>,
): Set<number> {
  const chipUs = pxToUs(CHIP_MIN_PX, pps);
  const bareUs = pxToUs(CUT_MIN_PX, pps);
  const out = new Set<number>();
  for (let i = Math.max(0, first - 1); i < Math.min(end, main.length - 1); i++) {
    const minUs = main[i].transitionOut ? chipUs : bareUs;
    if (index.ends[i] - index.starts[i] >= minUs && index.ends[i + 1] - index.starts[i + 1] >= minUs) out.add(i);
  }
  for (const id of keep) {
    const clipId = cutClipId(id) ?? transitionClipId(id);
    const i = clipId === null ? -1 : clipIndexOf(main, clipId);
    if (i >= 0 && i + 1 < main.length) out.add(i);
  }
  return out;
}

/** How wide (px) clip `i`'s half of a transition draws as a tint at its edge: 0 when there's none or its chip draws. */
function tintPx(main: Clip[], i: number, side: 'in' | 'out', chips: ReadonlySet<number>, pps: number): number {
  const at = side === 'out' ? i : i - 1;
  const transition = at >= 0 && at + 1 < main.length ? main[at].transitionOut : undefined;
  if (!transition || chips.has(at)) return 0;
  const [before, after] = splitDuration(transition.durationUs);
  return Math.max(2, usToPx(side === 'out' ? before : after, pps));
}

interface MainLaneProps {
  main: Clip[];
  assets: Asset[];
  recordings: Recording[];
  /** The project's frame rate: a new transition is whole frames long. */
  fps: number;
  pps: number;
  /** The time to draw (the view plus a margin, from `useViewport`). */
  fromUs: number;
  toUs: number;
  /** A clip's inside (px): the Video lane takes the timeline's spare height. */
  innerPx?: number;
  selectedId: string | null;
  /** The block in the Tab order (roving focus). */
  tabStopId: string | null;
  /** Blocks that stay mounted and never band: selected, focused, dragged. */
  keep: ReadonlySet<string>;
  onPick: (id: string) => void;
  onPress: PressBlock;
  onTrimPress: PressTrim;
}

/**
 * The main track's clips in the window, back to back, with a 1 px seam where two meet; clips narrower than a clip
 * block draws merge into bands. Its props keep their identity across an edit that leaves the main track alone, so
 * such an edit doesn't even re-render the lane. The cuts between clips show their transitions, or offer one
 * (`CutMarker`); while a block is dragged they don't draw (they would stay where the cuts were).
 */
export const MainLane = memo(function MainLane({
  main,
  assets,
  recordings,
  fps,
  pps,
  fromUs,
  toUs,
  innerPx = CLIP_INNER_PX,
  selectedId,
  tabStopId,
  keep,
  onPick,
  onPress,
  onTrimPress,
}: MainLaneProps) {
  // Labels like "Missing media" are in the language shown: a switch re-renders the lane.
  useLocale();
  const dragging = useDragged((s) => s.ids.length > 0);
  const index = mainIndex(main);
  const [first, end] = visibleRange(index, fromUs, toUs);
  const sources = { assets, recordings };
  const bands: ReactNode[] = [];
  const blocks: ReactNode[] = [];
  const idAt = (i: number) => main[i].id;
  const pieces = mergeTiny(index, first, end, pps, CLIP_MIN_PX, (i) => keep.has(idAt(i)));
  // Worked out whether or not a drag hides the markers, so a lift never changes the clips' tints (or re-renders them).
  const cutsAt = drawnCuts(main, index, first, end, pps, keep);
  for (const piece of withKept(pieces, main.length, first, end, keep, idAt)) {
    if (piece.kind === 'band') {
      bands.push(
        <Band
          key={`band:${piece.first}`}
          left={usToPx(piece.startUs, pps)}
          width={usToPx(piece.endUs - piece.startUs, pps)}
          look="video"
        />,
      );
      continue;
    }
    const i = piece.index;
    const clip = main[i];
    blocks.push(
      <ClipBlock
        key={clip.id}
        id={clip.id}
        label={sourceLabel(sources, clip.source)}
        position={i + 1}
        count={main.length}
        startUs={index.starts[i]}
        durationUs={index.ends[i] - index.starts[i]}
        files={clipMediaFiles(sources, clip)}
        sourceInUs={clip.sourceInUs}
        speed={clip.speed}
        volume={clip.volume}
        fadeInUs={clip.fadeInUs}
        fadeOutUs={clip.fadeOutUs}
        muted={clip.muted}
        transitionInPx={tintPx(main, i, 'in', cutsAt, pps)}
        transitionOutPx={tintPx(main, i, 'out', cutsAt, pps)}
        innerPx={innerPx}
        pps={pps}
        selected={clip.id === selectedId}
        tabbable={clip.id === tabStopId}
        onPick={onPick}
        onPress={onPress}
        onTrimPress={onTrimPress}
      />,
    );
  }
  const cuts: ReactNode[] = [];
  if (!dragging) {
    const footage: Footage = { assets, recordings, timeline: { main } };
    for (const i of cutsAt) {
      const clip = main[i];
      const transition = clip.transitionOut;
      cuts.push(
        <CutMarker
          key={cutId(clip.id)}
          clipId={clip.id}
          cutUs={index.ends[i]}
          pps={pps}
          before={sourceLabel(sources, clip.source)}
          after={sourceLabel(sources, main[i + 1].source)}
          transitionName={transition ? transitionName(transition) : null}
          transitionKind={transition ? transitionKind(transition) : ''}
          transitionUs={transition?.durationUs ?? 0}
          transitionTitle={transition && !isInstalled(transition.effect) ? transition.effect : undefined}
          canAdd={!transition && defaultTransitionUs(footage, i, fps) > 0}
          replaces={transition ? 'none' : fadesReplaced(main, i)}
          selected={transitionClipId(selectedId) === clip.id}
          tabbable={tabStopId === cutId(clip.id)}
          onAdd={addTransition}
          onSelect={pickTransition}
          onNoRoom={noRoomAt}
        />,
      );
    }
  }
  // Bands first: a kept clip drawn wider than its time (16 px) stays on top of the band after it. The cut markers
  // last, over the clips' edges.
  return [...bands, ...blocks, ...cuts, <CutTip key="cut-tip" />];
});
