import type { Size } from '../../lib/preview';

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
