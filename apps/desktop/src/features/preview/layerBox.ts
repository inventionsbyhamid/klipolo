import type { CSSProperties } from 'react';
import type { BoxN } from '../../bindings/BoxN';
import type { Item } from '../../bindings/Item';
import type { Size } from '../../lib/preview';

/** Shift by (dx, dy) CSS px, then rotate about the centre (the engine's rotation: degrees clockwise). */
export function boxTransform(dx: number, dy: number, rotation: number): string {
  const shift = dx || dy ? `translate(${dx}px, ${dy}px)` : '';
  const turn = rotation ? `rotate(${rotation}deg)` : '';
  return shift && turn ? `${shift} ${turn}` : shift || turn;
}

/** Where a box sits on the stage, in CSS pixels, rotated about its centre (pixels and hit area alike). */
export function boxStyle(rect: BoxN, canvas: Size, rotation: number): CSSProperties {
  return {
    left: rect.x * canvas.width,
    top: rect.y * canvas.height,
    width: rect.w * canvas.width,
    height: rect.h * canvas.height,
    transform: boxTransform(0, 0, rotation) || undefined,
  };
}

export function layerBox(item: Item, rect: BoxN, canvas: Size): CSSProperties {
  return boxStyle(rect, canvas, item.layout.rotation);
}
