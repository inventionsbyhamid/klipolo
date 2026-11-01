import { plural, t } from '@lingui/core/macro';
import { memo } from 'react';
import type { Asset } from '../../../bindings/Asset';
import type { Item } from '../../../bindings/Item';
import type { Recording } from '../../../bindings/Recording';
import type { Track } from '../../../bindings/Track';
import { Tooltip } from '../../../design/Tooltip';
import { useLocale } from '../../../i18n/react';
import { itemLabel } from '../../../lib/timeline';
import { pickBlock } from '../drag/blockPress';
import type { PressBlock, PressTrim } from '../drag/useBlockDrag';
import { HEADER_PX, LAYER_ROW_PX, LAYERS_PAD_PX } from '../layout';
import { packRows } from '../model/packRows';
import { usToPx } from '../scale';
import { linePieces, type Box, type Pieces } from './TrackLane';

/** "+N" markers closer than this (px) are counted together, so a zoomed-out lane shows a few, not hundreds. */
const MARKER_PX = 24;

/** A "+N" marker: where it draws (px), the time its runs span (µs), and the selected layer among them. */
interface Marker {
  left: number;
  right: number;
  count: number;
  fromUs: number;
  toUs: number;
  /** Its first hidden layer: with the selected one, what keys it, so a zoom or a scroll never re-mounts it. */
  firstId: string;
  selected: Item | null;
}

interface LayersLaneProps {
  tracks: Track[];
  assets: Asset[];
  recordings: Recording[];
  pps: number;
  fromUs: number;
  toUs: number;
  selectedId: string | null;
  selection: ReadonlySet<string>;
  tabStopId: string | null;
  keep: ReadonlySet<string>;
  onPick: (id: string) => void;
  onPress: PressBlock;
  onTrimPress: PressTrim;
}

/** Where packed row `row`'s layers draw (the chat's ghosted timeline outlines them there too). */
export function packedBox(row: number): Box {
  return { top: LAYERS_PAD_PX / 2 + row * LAYER_ROW_PX, height: LAYER_ROW_PX - 2 };
}

/** The lane's height for `packing.rows` rows: 24 px for one row, 44 for two, 64 for three. */
export function layersLanePx(tracks: Track[]): number {
  return LAYERS_PAD_PX + Math.max(1, packRows(tracks).rows) * LAYER_ROW_PX;
}

/**
 * The Overlays lane: every picture layer of every overlay and zoom track, packed into at most three
 * rows. Sub-pixel runs merge per packed row, so a band stays in its row and never covers a block. Layers
 * that don't fit are counted in a "+N" marker, which selects the one it stands for (the selected one among them, else
 * the first) so the preview and the keys reach it.
 */
export const LayersLane = memo(function LayersLane({ tracks, assets, recordings, ...view }: LayersLaneProps) {
  // The "+N" names are in the language shown: a switch re-renders the lane. (The core `t`, not the hook's: the native
  // macro drops a `plural` inside the hook's `t` in a `memo(function …)` component, @lingui/native-tools 0.1.6.)
  useLocale();
  const packing = packRows(tracks);
  const line = { ...view, sources: { assets, recordings } };
  const out: Pieces = { bands: [], blocks: [] };
  packing.lines.forEach(({ items, tracks: on }, r) =>
    linePieces(items, (i) => on[i], packedBox(r), `row${r}`, line, out),
  );
  // Runs of hidden layers in the window, nearby ones counted together. A run that began left of the window still
  // shows: its marker spans the run and sticks to the lanes' left edge while the run is on screen. A marker holding
  // the selected layer names it: focus moves there when the focused layer is packed away (`useRovingFocus`).
  const markers: Marker[] = [];
  for (const run of packing.overflow) {
    if (run.endUs <= view.fromUs || run.startUs >= view.toUs) continue;
    const left = usToPx(run.startUs, view.pps);
    const right = usToPx(run.endUs, view.pps);
    const selected =
      view.selectedId === null ? null : (run.items.find((i) => i.id === view.selectedId) ?? null);
    const last = markers.at(-1);
    if (last && left < last.right + MARKER_PX) {
      last.right = Math.max(last.right, right);
      last.count += run.count;
      last.toUs = Math.max(last.toUs, run.endUs);
      last.selected ??= selected;
    } else
      markers.push({
        left,
        right,
        count: run.count,
        fromUs: run.startUs,
        toUs: run.endUs,
        firstId: run.items[0].id,
        selected,
      });
  }
  const box = packedBox(packing.rows - 1);
  return [
    ...out.bands,
    ...out.blocks,
    ...markers.map(({ left, right, count, fromUs, toUs, firstId, selected }) => {
      const label = selected ? itemLabel(line.sources, selected) : '';
      return (
        <div
          // By the layers it hides, not where it draws: a focused marker stays the same element through a zoom.
          key={`more:${selected?.id ?? firstId}`}
          className="pointer-events-none absolute z-[6] flex"
          style={{ left, width: right - left, top: box.top, height: box.height }}
        >
          <Tooltip
            label={t({
              id: 'timeline.moreLayers.tip',
              message: plural(count, {
                one: '# more layer here. Click to select it',
                other: '# more layers here. Click to select one',
              }),
            })}
          >
            <button
              type="button"
              data-more-from={fromUs}
              data-more-to={toUs}
              // The visible "+N" starts the name, so voice control can say what it sees (WCAG 2.5.3).
              aria-label={
                selected
                  ? t({
                      id: 'timeline.moreLayers.nameSelected',
                      message: plural(count, {
                        one: `+# more layer: ${label}, selected`,
                        other: `+# more layers, including ${label}, selected`,
                      }),
                      comment:
                        'Starts with the visible “+N”; {label} is the hidden layer that is selected (⌫ deletes it)',
                    })
                  : t({
                      id: 'timeline.moreLayers.name',
                      message: plural(count, {
                        one: '+# more layer, select it',
                        other: '+# more layers, select the first',
                      }),
                      comment: 'Starts with the visible “+N” so voice control can say what it sees',
                    })
              }
              onClick={(e) => {
                e.stopPropagation();
                pickBlock(selected?.id ?? firstId);
              }}
              style={{ left: HEADER_PX }}
              className="tl-more"
            >
              +{count}
            </button>
          </Tooltip>
        </div>
      );
    }),
  ];
});
