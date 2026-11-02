import type { Size } from '../../lib/preview';

/**
 * The largest box with the given aspect ratio that fits inside `container`: the video in the stage's area. Only the
 * area and the format decide it, never what is selected (the card shows in the left column, `Library`), so selecting,
 * deselecting and walking the lanes never resize or move the video (or the engine's drawable).
 */
export function fitBox(container: Size, aspect: readonly [number, number]): Size {
  const ratio = aspect[0] / aspect[1];
  const width = Math.max(0, Math.min(container.width, container.height * ratio));
  return { width: Math.round(width), height: Math.round(width / ratio) };
}
