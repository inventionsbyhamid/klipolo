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

/** A new video layer fits within this share of the frame's width and of its height. */
const VIDEO_LAYER_MAX = 0.4;
/** Kept clear between a new video layer and the edges: 4 % of the shorter side, as a layer placed on the grid. */
const VIDEO_LAYER_MARGIN = 0.04;

/**
 * A new video layer's box in `format`: the video's own shape, within 40 % of the frame's width and height (a video of
 * the frame's shape is 40 % wide), in the bottom right corner, `VIDEO_LAYER_MARGIN` in from the edges of what the apps
 * leave clear in 9:16 (the "all" safe area) and of the frame elsewhere. A video of unknown size gets the frame's shape.
 */
export function videoLayerRect(asset: Asset, format: FormatKind): BoxN {
  const [cw, ch] = FORMAT_SIZE[format];
  const area = safeRect(format, 'all') ?? { x: 0, y: 0, w: 1, h: 1 };
  const aspect = asset.width > 0 && asset.height > 0 ? asset.width / asset.height : cw / ch;
  let w = VIDEO_LAYER_MAX;
  let h = (w * cw) / (aspect * ch);
  if (h > VIDEO_LAYER_MAX) {
    h = VIDEO_LAYER_MAX;
    w = (h * ch * aspect) / cw;
  }
  const short = Math.min(cw, ch);
  const right = area.x + area.w - (VIDEO_LAYER_MARGIN * short) / cw;
  const bottom = area.y + area.h - (VIDEO_LAYER_MARGIN * short) / ch;
  return { x: right - w, y: bottom - h, w, h };
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
