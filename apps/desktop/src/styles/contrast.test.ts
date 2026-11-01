import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const css = readFileSync(resolve(__dirname, 'tokens.css'), 'utf8');

/** Colours the same in both themes: highlighter yellow, and the navy written on it. */
const SAME_IN_BOTH = ['hilite', 'hilite-hover', 'hilite-press', 'navy'];
/** Drawn by macOS, listed for reference only. */
const MAC_ONLY = ['mac-close', 'mac-min', 'mac-zoom'];

/** A colour token's light and dark values: `--color-name: light-dark(#light, #dark)`, or one `#hex` for both. */
function token(name: string): [string, string] {
  const pair = css.match(new RegExp(`--color-${name}:\\s*light-dark\\((#[0-9a-f]{6}),\\s*(#[0-9a-f]{6})\\)`, 'i'));
  if (pair) return [pair[1], pair[2]];
  const one = css.match(new RegExp(`--color-${name}:\\s*(#[0-9a-f]{6})\\s*;`, 'i'));
  if (one) return [one[1], one[1]];
  throw new Error(`no colour token --color-${name}`);
}

/** WCAG 2's relative luminance of `#rrggbb`. */
function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** Every surface text sits on: the window, panels, cards and inputs, hover and pressed rows, dialogs. */
const SURFACES = ['night', 'bay', 'bay-raised', 'bay-hover', 'bay-press', 'dialog'];

/** `fg` on each of `on` reaches `least` in the light theme and in the dark one. */
function expectContrast(fg: string, on: string[], least: number) {
  const [fgLight, fgDark] = token(fg);
  for (const surface of on) {
    const [light, dark] = token(surface);
    expect(contrast(fgLight, light), `${fg} on ${surface}, light`).toBeGreaterThanOrEqual(least);
    expect(contrast(fgDark, dark), `${fg} on ${surface}, dark`).toBeGreaterThanOrEqual(least);
  }
}

describe('colour tokens', () => {
  it('the page follows the system appearance, and data-theme forces one', () => {
    expect(css).toMatch(/:root\s*\{\s*color-scheme:\s*light dark;/);
    expect(css).toMatch(/\[data-theme=['"]light['"]\]\s*\{\s*color-scheme:\s*light;\s*\}/);
    expect(css).toMatch(/\[data-theme=['"]dark['"]\]\s*\{\s*color-scheme:\s*dark;\s*\}/);
  });

  it('every colour has a light and a dark value, except yellow and the navy on it', () => {
    const names = [...css.matchAll(/--color-([a-z0-9-]+):/g)].map((m) => m[1]);
    expect(names.length).toBeGreaterThan(30);
    for (const name of names) {
      if (SAME_IN_BOTH.includes(name) || MAC_ONLY.includes(name)) continue;
      expect(css, `--color-${name}`).toMatch(new RegExp(`--color-${name}:\\s*light-dark\\(`));
    }
  });

  it('text reaches AA (4.5:1) on every surface in both themes', () => {
    expectContrast('ink', SURFACES, 4.5);
    expectContrast('ink-soft', SURFACES, 4.5);
    // Meta text (times, counts) is never on a pressed row.
    expectContrast(
      'ink-faint',
      SURFACES.filter((s) => s !== 'bay-press'),
      4.5,
    );
  });

  it('text on a filled mark reaches AA in both themes', () => {
    // Selected pills, the play button, your chat bubbles: paper, with its own ink.
    expectContrast('on-paper', ['paper'], 4.5);
    // The primary action: navy on highlighter yellow.
    expectContrast('navy', ['hilite', 'hilite-hover'], 4.5);
    // "Cut" tags.
    expectContrast('on-cut', ['cut'], 4.5);
    // The music's name tag.
    expectContrast('on-lane', ['lane-music'], 4.5);
  });

  it('coloured text reaches AA on the panels it is written on', () => {
    expectContrast('cut-text', ['bay', 'bay-raised', 'dialog'], 4.5);
    // The music's ducking tag and the sound effects' names, beside their blocks.
    expectContrast('lane-music', ['bay'], 4.5);
    expectContrast('lane-sfx', ['bay'], 4.5);
    // A clip's name over its own colour.
    expectContrast('ink', ['clip-deep'], 4.5);
  });

  it('the focus ring and the borders of controls reach 3:1 in both themes', () => {
    expectContrast('paper', SURFACES, 3);
    expectContrast('line-strong', ['night', 'bay', 'bay-raised', 'dialog'], 3);
  });
});
