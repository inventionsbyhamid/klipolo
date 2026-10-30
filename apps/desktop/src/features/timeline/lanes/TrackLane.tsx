import type { ReactNode } from 'react';
import type { Item } from '../../../bindings/Item';
import type { Track } from '../../../bindings/Track';
import { itemLabel, itemTitle, sourceLabel, type Sources } from '../../../lib/timeline';
import { ItemBlock, type ItemLook } from '../blocks/ItemBlock';
import type { PressBlock, PressTrim } from '../drag/useBlockDrag';
import { LAYER_MIN_PX, SOUND_MIN_PX } from '../layout';
import { itemFiles } from '../media/blockFiles';
import { itemSpans } from '../model/index';
import { mergeTiny, runBands, visibleRange, withKept } from '../model/visible';
import { usToPx } from '../scale';
import { Band, bandMask } from './Band';

/** Where a block or band draws within its lane (px). */
export interface Box {
  top: number;
  height: number;
}

export interface LineView {
  sources: Sources;
  pps: number;
  fromUs: number;
  toUs: number;
  selectedId: string | null;
  /** The block in the Tab order (roving focus). */
  tabStopId: string | null;
  /** Blocks that stay mounted and never band: selected, focused, dragged. */
  keep: ReadonlySet<string>;
  onPick: (id: string) => void;
  onPress: PressBlock;
  onTrimPress: PressTrim;
}

/** A lane's pieces: bands are drawn first, so no band is ever over a block. */
export interface Pieces {
  bands: ReactNode[];
  blocks: ReactNode[];
}

/**
 * One line of layers that never overlap (a packed row of the Overlays lane, or the Sounds or Music lane) in the
 * window, drawn as `look`: blocks, with runs
 * narrower than a layer block draws merged into bands, and the line's bands that no block
 * interrupts drawn as one element (`runBands`). `trackOf(i)` is the track `items[i]` is on;
 * `key` keeps the line's bands apart from other lines'. A kept layer (selected, focused, dragged) always stays a
 * mounted block, even outside the window.
 */
export function linePieces(
  items: Item[],
  trackOf: (index: number) => Track,
  box: Box,
  key: string,
  o: LineView,
  out: Pieces,
  look?: ItemLook,
) {
  const spans = itemSpans(items);
  const [first, end] = visibleRange(spans, o.fromUs, o.toUs);
  const idAt = (i: number) => items[i].id;
  const minPx = look === 'sound' || look === 'music' ? SOUND_MIN_PX : LAYER_MIN_PX;
  const pieces = mergeTiny(spans, first, end, o.pps, minPx, (i) => o.keep.has(idAt(i)));
  for (const piece of runBands(withKept(pieces, items.length, first, end, o.keep, idAt))) {
    if (piece.kind === 'bands') {
      const left = usToPx(piece.startUs, o.pps);
      const parts = piece.parts.map(([a, b]) => [usToPx(a, o.pps) - left, usToPx(b, o.pps) - left] as const);
      out.bands.push(
        <Band
          key={`band:${key}:${piece.first}`}
          left={left}
          width={usToPx(piece.endUs - piece.startUs, o.pps)}
          top={box.top}
          height={box.height}
          look={look ?? 'overlay'}
          mask={bandMask(parts)}
        />,
      );
      continue;
    }
    const item = items[piece.index];
    const track = trackOf(piece.index);
    out.blocks.push(
      <ItemBlock
        key={item.id}
        id={item.id}
        label={itemLabel(o.sources, item)}
        kind={item.content.type}
        title={itemTitle(item)}
        startUs={item.startUs}
        durationUs={item.durationUs}
        content={item.content}
        files={itemFiles(o.sources, item.content)}
        muted={track.kind === 'audio' && track.muted}
        pps={o.pps}
        selected={item.id === o.selectedId}
        tabbable={item.id === o.tabStopId}
        dimmed={track.hidden || (track.kind === 'audio' && track.muted)}
        locked={track.locked}
        attachedTo={item.attach ? sourceLabel(o.sources, item.attach.source) : null}
        top={box.top}
        height={box.height}
        look={look}
        onPick={o.onPick}
        onPress={o.onPress}
        onTrimPress={o.onTrimPress}
      />,
    );
  }
}

/** A row's blocks and bands fill it, `inset` px in from its top and bottom edges. */
export function rowBox(rowPx: number, inset = 4): Box {
  return { top: inset, height: rowPx - 2 * inset };
}
