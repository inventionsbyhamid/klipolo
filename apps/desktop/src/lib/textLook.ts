import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import type { CSSProperties } from 'react';
import type { TextStyle } from '../bindings/TextStyle';

/**
 * The caption looks a text layer can wear, by key and name. Undo labels name a look by its message id
 * (`{ style: { code: 'text.look.box' } }`, `labels.ts`); the look's colours and outline come with the text tools.
 */
export interface TextLookPreset {
  key: string;
  name: MessageDescriptor;
}

export const CAPTION_LOOKS: TextLookPreset[] = [
  {
    key: 'clean',
    name: msg({
      id: 'text.look.clean',
      message: 'Clean',
      comment: 'Caption style: white text with a shadow',
    }),
  },
  {
    key: 'boldOutline',
    name: msg({ id: 'text.look.boldOutline', message: 'Bold outline', comment: 'Caption style' }),
  },
  {
    key: 'highlight',
    name: msg({
      id: 'text.look.highlight',
      message: 'Highlight',
      comment: 'Caption style: text on a yellow band',
    }),
  },
  {
    key: 'box',
    name: msg({ id: 'text.look.box', message: 'Box', comment: 'Caption style: white text in a black box' }),
  },
  {
    key: 'pop',
    name: msg({ id: 'text.look.pop', message: 'Pop', comment: 'Caption style: yellow text with an outline' }),
  },
];

const round2 = (v: number) => Math.round(v * 100) / 100;

/** The style at font `size`, its outline scaled with it. */
export function scaledStyle(style: TextStyle, size: number): TextStyle {
  const outline = style.outline;
  const k = size / style.size;
  return { ...style, size, outline: outline ? { ...outline, width: round2(outline.width * k) } : outline };
}

/** WCAG 2's relative luminance of `#rrggbb` (0 black to 1 white); 1 for anything else. */
function lightness(hex: string): number {
  if (!/^#[0-9a-f]{6}/i.test(hex)) return 1;
  const [r, g, b] = [1, 3, 5].map((i) => {
    const s = parseInt(hex.slice(i, i + 2), 16) / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/**
 * The caret typing a title on the video: one that shows where the letters' own colour may not (a white title over a
 * light recording). The outline's colour when there is one, else dark beside light letters and light beside dark ones.
 */
export function caretColor(style: Pick<TextStyle, 'color' | 'outline'>): string {
  if (style.outline) return style.outline.color;
  // 0.179: where black and white contrast with a colour equally.
  return lightness(style.color) > 0.179 ? '#000000' : '#FFFFFF';
}

/*
 * How the engine draws text, in ems of the font size (crates/klipolo-engine/src/render/text.rs): lines 1.2 em apart; a
 * background box 0.2 em around the lines with 0.15 em corners; an outline of copies of the letters on rings out to its
 * width; and a drop shadow, one hard copy 0.02 em right and 0.055 em down in black at 0.35 x 0.85, falling from the
 * outline when there is one. The shadow mirrors the engine's pixels, so it's data.
 */
const LINE_HEIGHT = 1.2;
const BOX_PAD = 0.2;
const BOX_RADIUS = 0.15;
const SHADOW_DX = 0.02;
const SHADOW_DY = 0.055;
const SHADOW_COLOR = `rgb(0 0 0 / ${0.35 * 0.85})`;

/** Eight compass directions at `r`, like the engine's `ring`. */
function ring(r: number): [number, number][] {
  const d = r * Math.SQRT1_2;
  return [
    [r, 0],
    [-r, 0],
    [0, r],
    [0, -r],
    [d, d],
    [-d, d],
    [d, -d],
    [-d, -d],
  ];
}

/** Rings out to `r` (one per 2.5 px, at most five), like the engine's `stroke_offsets`. */
function strokeOffsets(r: number): [number, number][] {
  const rings = Math.min(5, Math.max(1, Math.ceil(r / 2.5)));
  return Array.from({ length: rings }, (_, i) => ring((r * (i + 1)) / rings)).flat();
}

const at = ([x, y]: [number, number], color: string) => `${round2(x)}px ${round2(y)}px ${color}`;

/**
 * CSS that draws `style` at a font of `px` CSS pixels the way the engine does (add the font family). It is an inline
 * block, so a background hugs the lines; its padding is taken back by a margin, so the text sits where it would
 * without one.
 */
export function textLookCss(style: TextStyle, px: number): CSSProperties {
  const outline = style.outline;
  const stroke = outline && style.size > 0 ? (outline.width / style.size) * px : 0;
  const rings = outline && stroke > 0 ? strokeOffsets(stroke) : [];
  // CSS paints the first shadow on top: the outline goes over the drop shadow, as the engine draws them.
  const shadows = outline ? rings.map((o) => at(o, outline.color)) : [];
  if (style.shadow) {
    const copies: [number, number][] = [[0, 0], ...rings];
    for (const [x, y] of copies) shadows.push(at([x + px * SHADOW_DX, y + px * SHADOW_DY], SHADOW_COLOR));
  }
  return {
    display: 'inline-block',
    fontSize: px,
    fontWeight: style.weight,
    lineHeight: LINE_HEIGHT,
    color: style.color,
    textShadow: shadows.length > 0 ? shadows.join(', ') : undefined,
    ...(style.background && {
      background: style.background,
      padding: `${BOX_PAD}em`,
      margin: `-${BOX_PAD}em`,
      borderRadius: `${BOX_RADIUS}em`,
    }),
  };
}
