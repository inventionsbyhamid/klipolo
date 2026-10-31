import type { Anchor } from '../bindings/Anchor';
import type { Asset } from '../bindings/Asset';
import type { BoxN } from '../bindings/BoxN';
import type { FormatKind } from '../bindings/FormatKind';
import type { Item } from '../bindings/Item';
import type { Layout } from '../bindings/Layout';
import type { Op } from '../bindings/Op';
import { newClipFor, newId } from './defaults';
import { FORMAT_SIZE } from './layout';
import { safeRect } from './safeAreas';

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

/** Kept clear around a layer placed at a grid position: 4 % of the shorter side. */
const MARGIN = 0.04;

/**
 * The box `rect` (in `format`) moved to grid position `anchor` (the inspector's Position grid), `MARGIN` in from the
 * edges (in 9:16, from the safe area's edges), its size kept.
 */
export function placedAt(rect: BoxN, anchor: Anchor, format: FormatKind, canvas: [number, number]): BoxN {
  const [w, h] = canvas;
  const short = Math.min(w, h);
  const area = safeRect(format, 'all') ?? { x: 0, y: 0, w: 1, h: 1 };
  const mx = (MARGIN * short) / w;
  const my = (MARGIN * short) / h;
  const left = area.x + mx;
  const top = area.y + my;
  const right = area.x + area.w - mx - rect.w;
  const bottom = area.y + area.h - my - rect.h;
  const col = anchor.endsWith('Left') ? 0 : anchor.endsWith('Right') ? 2 : 1;
  const row = anchor.startsWith('top') ? 0 : anchor.startsWith('bottom') ? 2 : 1;
  const x = col === 0 ? left : col === 2 ? right : area.x + (area.w - rect.w) / 2;
  const y = row === 0 ? top : row === 2 ? bottom : area.y + (area.h - rect.h) / 2;
  return { ...rect, x, y };
}

/** A new video layer fits within this share of the frame's width and of its height. */
const VIDEO_LAYER_MAX = 0.4;

/**
 * A new video layer's box in `format`: the video's own shape, within 40 % of the frame's width and height (a video of
 * the frame's shape is 40 % wide), at the Position grid's Bottom right (`placedAt`: inside the safe area in 9:16). A
 * video of unknown size gets the frame's shape.
 */
export function videoLayerRect(asset: Asset, format: FormatKind): BoxN {
  const [cw, ch] = FORMAT_SIZE[format];
  const aspect = asset.width > 0 && asset.height > 0 ? asset.width / asset.height : cw / ch;
  let w = VIDEO_LAYER_MAX;
  let h = (w * cw) / (aspect * ch);
  if (h > VIDEO_LAYER_MAX) {
    h = VIDEO_LAYER_MAX;
    w = (h * ch * aspect) / cw;
  }
  return placedAt({ x: 0, y: 0, w, h }, 'bottomRight', format, FORMAT_SIZE[format]);
}

/**
 * A new video layer's layout (a picture in picture, not a cover over the main video): `videoLayerRect` in `primary`,
 * and its own place in each other format, as a title gets, so it sits in the corner of every format it is shown in.
 */
export function videoLayerLayout(asset: Asset, primary: FormatKind): Layout {
  const overrides: Layout['overrides'] = {};
  for (const format of Object.keys(FORMAT_SIZE) as FormatKind[])
    if (format !== primary) overrides[format] = videoLayerRect(asset, format);
  return { ...layoutFor(videoLayerRect(asset, primary)), anchor: 'bottomRight', overrides };
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
 * Ops that bring freshly imported media into the project: videos join the end of the main track in the order given;
 * sounds and pictures go to the library (the project's files), never the main track, for the person to place from
 * there (a drag, the "+", a drop on a lane). One `addAssets` for all of them.
 */
export function placementOps(assets: Asset[]): Op[] {
  const usable = assets.filter(placeable);
  if (usable.length === 0) return [];
  const clips = usable.filter((asset) => asset.kind === 'video').map(newClipFor);
  return [
    { type: 'addAssets', assets: usable },
    ...clips.map((clip): Op => ({ type: 'insertClip', clip, index: null })),
  ];
}
