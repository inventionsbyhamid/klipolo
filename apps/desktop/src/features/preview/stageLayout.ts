import type { BoxN } from '../../bindings/BoxN';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Project } from '../../bindings/Project';
import { cameraRect, resolveRect } from '../../lib/layout';
import type { Size } from '../../lib/preview';
import { findClip, findItem } from '../../lib/timeline';
import { cameraClipId } from '../../state/selection';

/**
 * The share of its width the video must keep beside a slot of the card's own. Below it the video keeps all its size and
 * the card floats over its right side.
 */
export const SLOT_KEEPS = 0.8;

/** Where the video and the card go in the stage's area. */
export interface StageLayout {
  /** The card's place: a slot of its own at the area's right, beside the video, or over the video's right side. */
  card: 'slot' | 'over';
  /** CSS px kept clear at the area's right: the video is centred in the rest. */
  pad: number;
  /** The video's box (CSS px). */
  canvas: Size;
}

/** The largest box with the given aspect ratio that fits inside `container`. */
export function fitBox(container: Size, aspect: readonly [number, number]): Size {
  const ratio = aspect[0] / aspect[1];
  const width = Math.max(0, Math.min(container.width, container.height * ratio));
  return { width: Math.round(width), height: Math.round(width / ratio) };
}

/**
 * The video and the card in an `area` for a video of `aspect`, with `slot` the card's width and the gap before it. Only
 * these decide it, never what is selected: selecting, deselecting and walking the lanes never resize or move the video
 * (or the engine's drawable). The card gets a slot of its own while the video keeps `SLOT_KEEPS` of its width beside it;
 * then the video stays centred in the whole area if the slot fits at its right, and otherwise moves left only as far as
 * the slot needs. Else the video keeps its whole size and the card floats over it.
 */
export function stageLayout(area: Size, aspect: readonly [number, number], slot: number): StageLayout {
  const full = fitBox(area, aspect);
  const beside = fitBox({ width: Math.max(0, area.width - slot), height: area.height }, aspect);
  if (full.width === 0 || beside.width < full.width * SLOT_KEEPS) return { card: 'over', pad: 0, canvas: full };
  const pad = Math.min(slot, Math.max(0, beside.width + 2 * slot - area.width));
  return { card: 'slot', pad, canvas: beside };
}

/** The most of the stage's height a card over the video takes; its fields scroll inside it. */
export const CARD_MAX = 0.6;

/** The stage's corner a card over the video goes to. */
export type CardAnchor = 'top-right' | 'bottom-right' | 'top-left' | 'bottom-left';

/** Where the selected thing is drawn on the video in `format`: a layer's box, a camera's picture; null for the rest. */
export function selectedBox(project: Project, selectedId: string | null, format: FormatKind): BoxN | null {
  if (!selectedId) return null;
  const primary = project.settings.primaryFormat;
  const cameraOf = cameraClipId(selectedId);
  if (cameraOf !== null) {
    const camera = findClip(project, cameraOf)?.camera;
    return camera ? cameraRect(camera, project.background.padding, primary, format) : null;
  }
  const found = findItem(project, selectedId);
  const type = found?.item.content.type;
  if (!found || (type !== 'text' && type !== 'image' && type !== 'video' && type !== 'generator')) return null;
  return resolveRect(found.item.layout, primary, format);
}

/** How many square px of `b` the box `a` covers. */
function covered(a: BoxN, b: BoxN): number {
  const w = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
  const h = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
  return w > 0 && h > 0 ? w * h : 0;
}

/**
 * The stage's corner for a card over the video that covers the selected layer's `box` least: a card `cardWidth` wide
 * and up to `CARD_MAX` of the `area` tall, with the video's `canvas` centred in it. The right side first: the bottom
 * right for a layer whose centre is in the top half, the top right otherwise. The left side only when both right
 * corners would cover more of the layer than a left one; then the left corner that covers least (the same top or
 * bottom on a tie). With nothing placed on the video, the top right.
 */
export function cardAnchor(box: BoxN | null, area: Size, canvas: Size, cardWidth: number): CardAnchor {
  if (!box) return 'top-right';
  const layer = {
    x: (area.width - canvas.width) / 2 + box.x * canvas.width,
    y: (area.height - canvas.height) / 2 + box.y * canvas.height,
    w: box.w * canvas.width,
    h: box.h * canvas.height,
  };
  const h = area.height * CARD_MAX;
  const covers = (side: 'left' | 'right', edge: 'top' | 'bottom') =>
    covered(
      { x: side === 'left' ? 0 : area.width - cardWidth, y: edge === 'top' ? 0 : area.height - h, w: cardWidth, h },
      layer,
    );
  const edge = box.y + box.h / 2 < 0.5 ? 'bottom' : 'top';
  const other = edge === 'bottom' ? 'top' : 'bottom';
  const left = Math.min(covers('left', edge), covers('left', other));
  if (covers('right', 'top') > left && covers('right', 'bottom') > left)
    return covers('left', edge) <= covers('left', other) ? `${edge}-left` : `${other}-left`;
  return `${edge}-right`;
}
