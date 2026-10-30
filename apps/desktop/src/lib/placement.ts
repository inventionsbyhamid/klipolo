import type { Asset } from '../bindings/Asset';
import type { BoxN } from '../bindings/BoxN';
import type { Item } from '../bindings/Item';
import type { Layout } from '../bindings/Layout';
import type { Op } from '../bindings/Op';
import type { Project } from '../bindings/Project';
import { newClipFor, newId } from './defaults';
import { FORMAT_SIZE } from './layout';
import { mainDurationUs } from './timeline';

const MIN_US = 10_000;
/** An image layer's length when it is added. */
export const IMAGE_US = 5_000_000;
/** Imported images fit inside this share of the canvas width (and its full height). */
const IMAGE_MAX_W = 0.4;

export function layoutFor(rect: BoxN): Layout {
  return { anchor: 'center', rect, rotation: 0, opacity: 1, radius: 0, overrides: {} };
}

/** The largest centred box within the image limits that keeps the image's shape (no empty bands). */
export function imageRect(asset: Asset, cw: number, ch: number): BoxN {
  const aspect = Math.max(asset.width, 1) / Math.max(asset.height, 1);
  let w = IMAGE_MAX_W;
  let h = (w * cw) / (ch * aspect);
  if (h > 1) {
    h = 1;
    w = (ch * aspect) / cw;
  }
  return { x: (1 - w) / 2, y: (1 - h) / 2, w, h };
}

/** A sound layer of all of `asset`, free (attached to no clip), at 0. */
export function audioItem(asset: Asset): Item {
  return {
    id: newId(),
    startUs: 0,
    durationUs: asset.durationUs,
    attach: null,
    layout: layoutFor({ x: 0, y: 0, w: 1, h: 1 }),
    content: {
      type: 'audio',
      assetId: asset.id,
      sourceInUs: 0,
      volume: 1,
      fadeInUs: 0,
      fadeOutUs: 0,
      effect: null,
    },
    effects: [],
  };
}

/** Whether `asset` can go in the video: an image, or media at least 10 ms long. */
export function placeable(asset: Asset): boolean {
  return asset.kind === 'image' || asset.durationUs >= MIN_US;
}

/**
 * Ops that bring freshly imported media into the video: videos join the end of the main track,
 * audio goes on its own track from the start, images become 5-second layers at the playhead.
 * Clips go in first, so images imported with them attach to them.
 */
export function placementOps(assets: Asset[], project: Project, playheadUs: number): Op[] {
  const usable = assets.filter(placeable);
  if (usable.length === 0) return [];
  const clips = usable.filter((asset) => asset.kind === 'video').map(newClipFor);
  const ops: Op[] = [
    { type: 'addAssets', assets: usable },
    ...clips.map((clip): Op => ({ type: 'insertClip', clip, index: null })),
  ];
  // Images over the main footage follow it; past its end only layers play, so they stay where they are.
  const insideVideo = playheadUs < mainDurationUs([...project.timeline.main, ...clips]);
  const [cw, ch] = FORMAT_SIZE[project.settings.primaryFormat];
  for (const asset of usable) {
    if (asset.kind === 'audio') {
      ops.push({ type: 'addItem', track: { type: 'auto' }, attach: false, item: audioItem(asset) });
    } else if (asset.kind === 'image') {
      ops.push({
        type: 'addItem',
        track: { type: 'auto' },
        attach: insideVideo,
        item: {
          id: newId(),
          startUs: playheadUs,
          durationUs: IMAGE_US,
          attach: null,
          layout: layoutFor(imageRect(asset, cw, ch)),
          content: { type: 'image', assetId: asset.id },
          effects: [],
        },
      });
    }
  }
  return ops;
}
