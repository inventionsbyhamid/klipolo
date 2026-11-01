import { describe, expect, it } from 'vitest';
import type { TextStyle } from '../bindings/TextStyle';
import {
  CAPTION_LOOKS,
  lookPatch,
  outlineStep,
  outlineWidth,
  sameLook,
  scaledStyle,
  wearsLook,
} from './textLook';

const base: TextStyle = {
  font: { family: 'Inter', source: { type: 'bundled' } },
  size: 72,
  weight: 700,
  italic: false,
  color: '#FFFFFF',
  background: null,
  outline: null,
  shadow: true,
  align: 'center',
};

const look = (key: string) => CAPTION_LOOKS.find((l) => l.key === key)!;

describe('text looks', () => {
  it('an outline width is a share of the text size, so a step reads the same at any size', () => {
    expect(outlineWidth(72, 'thin')).toBe(2.88);
    expect(outlineWidth(72, 'medium')).toBe(5.04);
    expect(outlineWidth(72, 'thick')).toBe(7.2);
    expect(outlineStep(base)).toBeNull();
    expect(outlineStep({ ...base, outline: { color: '#000000', width: 2.88 } })).toBe('thin');
    // A width set elsewhere lands on the nearest step.
    expect(outlineStep({ ...base, outline: { color: '#000000', width: 6.5 } })).toBe('thick');
    // Scaled with the text, it keeps its step.
    const grown = scaledStyle({ ...base, outline: { color: '#000000', width: 5.04 } }, 144);
    expect(outlineStep(grown)).toBe('medium');
  });

  it('each caption look sets the colour, highlight, outline and shadow for the text’s size', () => {
    expect(lookPatch(look('clean'), 72)).toEqual({
      color: '#FFFFFF',
      background: null,
      outline: null,
      shadow: true,
    });
    expect(lookPatch(look('pop'), 72)).toEqual({
      color: '#FFD02E',
      background: null,
      outline: { color: '#000000', width: 5.04 },
      shadow: false,
    });
    expect(lookPatch(look('boldOutline'), 100).outline).toEqual({ color: '#000000', width: 10 });
  });

  it('a text wears a look when its colours, outline step and shadow match it', () => {
    expect(wearsLook(base, look('clean'))).toBe(true);
    expect(wearsLook(base, look('box'))).toBe(false);
    const popped = { ...base, ...lookPatch(look('pop'), 72) };
    expect(wearsLook(popped, look('pop'))).toBe(true);
    // Lower case digits are the same colour.
    expect(wearsLook({ ...popped, color: '#ffd02e' }, look('pop'))).toBe(true);
    expect(wearsLook({ ...popped, outline: { color: '#000000', width: 2.88 } }, look('pop'))).toBe(false);
  });

  it('two styles look the same when the colours, outline and shadow do', () => {
    const smaller: TextStyle = { ...base, size: 20 };
    expect(sameLook(base, smaller)).toBe(true);
    expect(sameLook(base, { ...base, color: '#ffffff' })).toBe(true);
    expect(sameLook(base, { ...base, shadow: false })).toBe(false);
    expect(sameLook(base, { ...base, background: '#000000' })).toBe(false);
    expect(sameLook(base, { ...base, outline: { color: '#000000', width: 1 } })).toBe(false);
  });
});
