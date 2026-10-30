import { memo, type ReactNode } from 'react';
import type { Asset } from '../../../bindings/Asset';
import type { Clip } from '../../../bindings/Clip';
import type { Recording } from '../../../bindings/Recording';
import { useLocale } from '../../../i18n/react';
import { sourceLabel } from '../../../lib/timeline';
import { ClipBlock } from '../blocks/ClipBlock';
import type { PressBlock, PressTrim } from '../drag/useBlockDrag';
import { CLIP_INNER_PX, CLIP_MIN_PX } from '../layout';
import { clipMediaFiles } from '../media/blockFiles';
import { mainIndex } from '../model/index';
import { mergeTiny, visibleRange, withKept } from '../model/visible';
import { usToPx } from '../scale';
import { Band } from './Band';

interface MainLaneProps {
  main: Clip[];
  assets: Asset[];
  recordings: Recording[];
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
 * such an edit doesn't even re-render the lane.
 */
export const MainLane = memo(function MainLane({
  main,
  assets,
  recordings,
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
  const index = mainIndex(main);
  const [first, end] = visibleRange(index, fromUs, toUs);
  const sources = { assets, recordings };
  const bands: ReactNode[] = [];
  const blocks: ReactNode[] = [];
  const idAt = (i: number) => main[i].id;
  const pieces = mergeTiny(index, first, end, pps, CLIP_MIN_PX, (i) => keep.has(idAt(i)));
  for (const piece of withKept(pieces, main.length, first, end, keep, idAt)) {
    if (piece.kind === 'band') {
      bands.push(
        <Band
          key={`band:${piece.first}`}
          left={usToPx(piece.startUs, pps)}
          width={usToPx(piece.endUs - piece.startUs, pps)}
          className="inset-y-0 bg-clip-deep"
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
  // Bands first: a kept clip drawn wider than its time (16 px) stays on top of the band after it.
  return [...bands, ...blocks];
});
