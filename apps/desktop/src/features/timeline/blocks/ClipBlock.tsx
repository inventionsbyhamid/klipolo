import { useLingui } from '@lingui/react/macro';
import { memo, useCallback } from 'react';
import { cn } from '../../../design/cn';
import { countRender } from '../../../lib/renderCount';
import { formatTime } from '../../../lib/time';
import { registerBlock } from '../drag/registry';
import {
  onBlockLostCapture,
  onBlockPointerCancel,
  onBlockPointerMove,
  onBlockPointerUp,
  type PressBlock,
  type PressTrim,
} from '../drag/useBlockDrag';
import { BLOCK_HINT_ID, CLIP_INNER_PX, CLIP_MIN_PX } from '../layout';
import { BlockMedia } from '../media/BlockMedia';
import type { BlockFiles } from '../media/blockFiles';
import { usToPx } from '../scale';
import { TrimHandles } from './TrimHandles';

interface ClipBlockProps {
  id: string;
  label: string;
  /** Its place on the main track (from 1) and the number of clips: "clip 3 of 12", for screen readers. */
  position: number;
  count: number;
  startUs: number;
  durationUs: number;
  /** The files its filmstrip and waveform draw from (`media/blockFiles`: the same object every render), and how. */
  files: BlockFiles | null;
  sourceInUs: number;
  speed: number;
  volume: number;
  fadeInUs: number;
  fadeOutUs: number;
  muted: boolean;
  /** Its inside (px), where the filmstrip and waveform draw: the Video lane less the clip's 1 px edges. */
  innerPx?: number;
  pps: number;
  selected: boolean;
  /** The one block in the Tab order (roving focus, `keys/useRovingFocus`). */
  tabbable: boolean;
  /** A keyboard click (Enter, Space) picks; a pointer picks on press, which also starts a drag. */
  onPick: (id: string) => void;
  onPress: PressBlock;
  onTrimPress: PressTrim;
}

/**
 * A main-track clip. Memoized on primitives: an edit elsewhere never re-renders it. It gets its own compositing
 * layer only while dragged (`will-change` on every clip would make hundreds of layers).
 */
export const ClipBlock = memo(function ClipBlock({
  id,
  label,
  position,
  count,
  startUs,
  durationUs,
  files,
  sourceInUs,
  speed,
  volume,
  fadeInUs,
  fadeOutUs,
  muted,
  innerPx = CLIP_INNER_PX,
  pps,
  selected,
  tabbable,
  onPick,
  onPress,
  onTrimPress,
}: ClipBlockProps) {
  countRender(id);
  const { t } = useLingui();
  const left = usToPx(startUs, pps);
  const width = usToPx(durationUs, pps);
  const duration = formatTime(durationUs);
  const start = formatTime(startUs);
  const ref = useCallback((el: HTMLButtonElement | null) => registerBlock(id, el), [id]);
  // Narrower than its padding and edge draw it (only a kept, selected clip is): its box is 16 px, and a real clip after
  // it covers the rest. So it keeps its contents in its own stacking context, and has no labels (cut to 6 px they read
  // as nothing) or trim handles (its end handle would be past its end): nothing of it draws over, or takes clicks from,
  // that clip.
  const tiny = width < CLIP_MIN_PX;
  return (
    <button
      type="button"
      tabIndex={tabbable ? 0 : -1}
      aria-describedby={BLOCK_HINT_ID}
      data-block-id={id}
      ref={ref}
      aria-label={t({
        id: 'timeline.clip.name',
        message: `${label}, clip ${position} of ${count}, ${duration}, starts at ${start}`,
        comment:
          'A clip for screen readers; {label} is its file name, {duration} and {start} are times like 1:05',
      })}
      aria-pressed={selected}
      onPointerDown={(e) => onPress(e, id)}
      onPointerMove={onBlockPointerMove}
      onPointerUp={onBlockPointerUp}
      onPointerCancel={onBlockPointerCancel}
      onLostPointerCapture={onBlockLostCapture}
      onClick={(e) => {
        // Never reaches the lane (which seeks). A pointer's click was its press, which already picked.
        e.stopPropagation();
        if (e.detail === 0) onPick(id);
      }}
      data-left={left}
      data-width={width}
      // Back to back with its neighbours: a seam (1 px night line) where two clips meet, round only at the ends.
      data-first={position === 1 || undefined}
      data-last={position === count || undefined}
      style={{ left, width }}
      className={cn('tl-clip', selected && 'is-selected', tiny && 'isolate')}
    >
      {/* Its filmstrip and waveform, first: the labels and trim handles draw over them. */}
      {!tiny && files && (
        <BlockMedia
          kind="clip"
          thumbPath={files.thumbPath}
          filmUs={files.thumbUs}
          peakPaths={files.peakPaths}
          peakLeadUs={files.peakLeadUs}
          runs={files.runs}
          startUs={startUs}
          durationUs={durationUs}
          sourceInUs={sourceInUs}
          speed={speed}
          volume={volume}
          fadeInUs={fadeInUs}
          fadeOutUs={fadeOutUs}
          muted={muted}
          pps={pps}
          height={innerPx}
        />
      )}
      {/* The name in a tag at the top left, over the filmstrip and the playhead line. */}
      {!tiny && (
        <>
          <span className="tl-clip-name">{label}</span>
          <TrimHandles selected={selected} onTrimPress={(e, edge) => onTrimPress(e, id, edge)} />
        </>
      )}
    </button>
  );
});
