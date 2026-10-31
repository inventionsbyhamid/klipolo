import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react/macro';
import { Film, Image, Link2, Lock, Music, Type, VolumeX, type LucideIcon } from 'lucide-react';
import { memo, useCallback } from 'react';
import type { ItemContent } from '../../../bindings/ItemContent';
import { cn } from '../../../design/cn';
import { layerRate } from '../../../lib/recording';
import { countRender } from '../../../lib/renderCount';
import { formatTime } from '../../../lib/time';
import type { LayerKind } from '../../../lib/timeline';
import { registerBlock } from '../drag/registry';
import {
  onBlockLostCapture,
  onBlockPointerCancel,
  onBlockPointerMove,
  onBlockPointerUp,
  type PressBlock,
  type PressTrim,
} from '../drag/useBlockDrag';
import { BLOCK_HINT_ID, LAYER_MIN_PX, SOUND_MIN_PX } from '../layout';
import { BlockMedia } from '../media/BlockMedia';
import type { BlockFiles } from '../media/blockFiles';
import { usToPx } from '../scale';
import { TrimHandles } from './TrimHandles';
import { VolumeLine } from './VolumeLine';

/** The icon before an overlay's name (T for text, the picture for an image). */
const ICON: Partial<Record<LayerKind, LucideIcon>> = { text: Type, image: Image, video: Film, audio: Music };

/** How a layer draws: on the Overlays lane, the Sounds lane (a teal block named to its right) or the Music lane. */
export type ItemLook = 'overlay' | 'sound' | 'music';

/** What screen readers call each kind of layer. */
const KIND_NAME: Record<LayerKind, MessageDescriptor> = {
  text: msg({ id: 'timeline.layer.kind.text', message: 'Text layer' }),
  video: msg({ id: 'timeline.layer.kind.video', message: 'Video layer' }),
  image: msg({ id: 'timeline.layer.kind.image', message: 'Image layer' }),
  audio: msg({ id: 'timeline.layer.kind.audio', message: 'Sound layer' }),
  zoom: msg({ id: 'timeline.layer.kind.zoom', message: 'Zoom layer' }),
  generator: msg({ id: 'timeline.layer.kind.generator', message: 'Generator layer' }),
  adjustment: msg({ id: 'timeline.layer.kind.adjustment', message: 'Adjustment layer' }),
};
const MUTED_SOUND = msg({ id: 'timeline.layer.kind.audioMuted', message: 'Muted sound layer' });

interface ItemBlockProps {
  id: string;
  label: string;
  kind: LayerKind;
  /** A tooltip (a missing generator's id). */
  title?: string;
  startUs: number;
  durationUs: number;
  /** What it shows, and the files its filmstrip or waveform draw from (`media/blockFiles`: the same object every
   * render). */
  content: ItemContent;
  files: BlockFiles | null;
  /** A sound layer on a muted track: its waveform is a flat line. */
  muted: boolean;
  pps: number;
  selected: boolean;
  /** The one block in the Tab order (roving focus, `keys/useRovingFocus`). */
  tabbable: boolean;
  /** The item's track is hidden (or muted, for audio): drawn faded. */
  dimmed: boolean;
  /** The item's track is locked: marked with a lock, and it can't be deleted, moved or trimmed. */
  locked: boolean;
  /** The label of the clip it moves with, when it's attached. */
  attachedTo: string | null;
  /** Its row within the lane (px). */
  top: number;
  height: number;
  /** The lane it draws on; by kind when not given (sound layers on Sounds, the rest on Overlays). */
  look?: ItemLook;
  /** A keyboard click (Enter, Space) picks; a pointer picks on press, which also starts a drag. */
  onPick: (id: string) => void;
  onPress: PressBlock;
  onTrimPress: PressTrim;
}

/**
 * A layer (text, image, video, sound, zoom, generator or adjustment) on an overlay, audio or zoom track. Memoized on
 * primitives, like `ClipBlock`: an edit elsewhere never re-renders it.
 */
export const ItemBlock = memo(function ItemBlock({
  id,
  label,
  kind,
  title,
  startUs,
  durationUs,
  content,
  files,
  muted,
  pps,
  selected,
  tabbable,
  dimmed,
  locked,
  attachedTo,
  top,
  height,
  look = kind === 'audio' ? 'sound' : 'overlay',
  onPick,
  onPress,
  onTrimPress,
}: ItemBlockProps) {
  countRender(id);
  const { t } = useLingui();
  const left = usToPx(startUs, pps);
  // Never narrower than its padding and edge draw it anyway, so `data-width` is what's on screen.
  const minPx = look === 'overlay' ? LAYER_MIN_PX : SOUND_MIN_PX;
  const width = Math.max(usToPx(durationUs, pps), minPx);
  // Narrower than that (only a kept, selected layer is), its box covers the start of the next layer in its row. As a
  // tiny clip does, it keeps its contents in its own stacking context and draws no label, icons or trim handles, so
  // nothing of it draws over, or takes clicks from, that layer (it comes later in the lane and draws on top).
  const tiny = usToPx(durationUs, pps) < minPx;
  const ref = useCallback((el: HTMLButtonElement | null) => registerBlock(id, el), [id]);
  // Muted by its own switch: an icon before its name (which gives way first as the block narrows), its sound faded.
  const soundMuted = content.type === 'audio' && !!content.muted;
  const layer = t(soundMuted ? MUTED_SOUND : KIND_NAME[kind]);
  const duration = formatTime(durationUs);
  const start = formatTime(startUs);
  const clip = attachedTo;
  const name =
    clip === null
      ? locked
        ? t({
            id: 'timeline.layer.nameLocked',
            message: `${layer}, ${label}, ${duration}, starts at ${start}, locked`,
            comment:
              'A layer for screen readers; {layer} is e.g. “Text layer”, {duration} and {start} are times',
          })
        : t({
            id: 'timeline.layer.name',
            message: `${layer}, ${label}, ${duration}, starts at ${start}`,
            comment:
              'A layer for screen readers; {layer} is e.g. “Text layer”, {duration} and {start} are times',
          })
      : locked
        ? t({
            id: 'timeline.layer.nameAttachedLocked',
            message: `${layer}, ${label}, ${duration}, starts at ${start}, moves with ${clip}, locked`,
            comment:
              'A layer for screen readers; {layer} is e.g. “Text layer”; {clip} is the clip it moves with',
          })
        : t({
            id: 'timeline.layer.nameAttached',
            message: `${layer}, ${label}, ${duration}, starts at ${start}, moves with ${clip}`,
            comment:
              'A layer for screen readers; {layer} is e.g. “Text layer”; {clip} is the clip it moves with',
          });
  const movesWith =
    clip === null
      ? undefined
      : t({
          id: 'timeline.layer.movesWith',
          message: `Moves with ${clip}`,
          comment: '{clip} is a clip’s name',
        });
  const tooltip = [title, movesWith].filter(Boolean).join('. ');
  const Icon = ICON[kind];
  return (
    <button
      type="button"
      tabIndex={tabbable ? 0 : -1}
      aria-describedby={BLOCK_HINT_ID}
      data-block-id={id}
      ref={ref}
      aria-label={name}
      aria-pressed={selected}
      title={tooltip || undefined}
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
      data-look={look}
      style={{ left, width, top, height }}
      className={cn('tl-item', `is-${look}`, selected && 'is-selected', dimmed && 'is-dimmed', tiny && 'isolate')}
    >
      {/* Music's waveform, or a video layer's filmstrip, first: its marks and label draw over it. */}
      {!tiny && files && look !== 'sound' && (content.type === 'video' || content.type === 'audio') && (
        <BlockMedia
          kind={content.type}
          thumbPath={files.thumbPath}
          filmUs={files.thumbUs}
          peakPaths={files.peakPaths}
          peakLeadUs={files.peakLeadUs}
          startUs={startUs}
          durationUs={durationUs}
          sourceInUs={content.sourceInUs}
          speed={content.type === 'video' ? content.speed : layerRate(content)}
          centreFrom={content.type === 'audio' ? content.centreFrom : null}
          volume={content.volume}
          fadeInUs={content.type === 'audio' ? content.fadeInUs : 0}
          fadeOutUs={content.type === 'audio' ? content.fadeOutUs : 0}
          muted={content.type === 'video' ? content.muted : muted}
          faded={soundMuted}
          pps={pps}
          height={height}
          waveToken={look === 'music' ? '--color-lane-music' : undefined}
        />
      )}
      {/* Music's volume line, its dips under speech and the tag on the piece that reaches the video's end. */}
      {!tiny && look === 'music' && content.type === 'audio' && (
        <VolumeLine
          id={id}
          startUs={startUs}
          durationUs={durationUs}
          content={content}
          pps={pps}
          height={height}
          tag={content.fadeOutUs > 0}
        />
      )}
      {!tiny && (
        <span className="tl-item-name">
          {Icon && look === 'overlay' && <Icon size={11} strokeWidth={1.8} aria-hidden />}
          {attachedTo !== null && <Link2 size={9} strokeWidth={2.4} aria-hidden />}
          {soundMuted && <VolumeX size={10} aria-hidden data-muted />}
          {locked && <Lock size={10} aria-hidden />}
          <span className="truncate">{label}</span>
        </span>
      )}
      {!tiny && !locked && (
        <TrimHandles selected={selected} onTrimPress={(e, edge) => onTrimPress(e, id, edge)} />
      )}
    </button>
  );
});
