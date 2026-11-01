import { describe, expect, it } from 'vitest';
import {
  alphaOf,
  hexToHsv,
  hsvToHex,
  keepAlpha,
  parseHex,
  rgbOf,
  sameColor,
  sameRgb,
  withAlpha,
} from './colorMath';

describe('parseHex', () => {
  it.each([
    ['ff0', false, '#FFFF00'],
    ['#ffd60a', false, '#FFD60A'],
    ['  #abc ', false, '#AABBCC'],
    ['FFD60A80', true, '#FFD60A80'],
    ['#FFD60A80', false, null],
    ['red', false, null],
    ['', false, null],
    ['#12345', false, null],
  ])('%j (alpha %s) → %j', (input, alpha, out) => expect(parseHex(input, alpha)).toBe(out));
});

it('compares colours whatever their case; no alpha digits is opaque; null equals only null', () => {
  expect(sameColor('#ffd60a', '#FFD60A')).toBe(true);
  expect([sameColor('#FF0000FF', '#ff0000'), sameColor('#FF000080', '#FF0000')]).toEqual([true, false]);
  expect([sameColor(null, null), sameColor(null, '#000000')]).toEqual([true, false]);
});

it('compares red, green and blue whatever the case and the alpha', () => {
  expect([sameRgb('#ffd60a80', '#FFD60A'), sameRgb('#FFD60A', '#FFD60B')]).toEqual([true, false]);
});

it('reads a colour’s red, green and blue', () => {
  expect(rgbOf('#FFD60A')).toEqual([255, 214, 10]);
  expect(rgbOf('#FFD60A80')).toEqual([255, 214, 10]);
});

it('converts HSV both ways and keeps a setting’s alpha', () => {
  expect(hexToHsv('#FF0000')).toEqual({ h: 0, s: 100, v: 100 });
  for (const hex of ['#FF9F0A', '#0A84FF', '#92979E']) expect(hsvToHex(hexToHsv(hex))).toBe(hex);
  expect([alphaOf('#FFD60A'), alphaOf('#FFD60A80'), withAlpha('#FFD60A', 50)]).toEqual([
    100,
    50,
    '#FFD60A80',
  ]);
  expect([
    keepAlpha('#FFFFFF', '#00000080'),
    keepAlpha('#FFFFFF', '#000000'),
    keepAlpha('#FFFFFF', null),
  ]).toEqual(['#FFFFFF80', '#FFFFFF', '#FFFFFF']);
  expect(keepAlpha('#FFFFFF40', '#00000080')).toBe('#FFFFFF40');
});
