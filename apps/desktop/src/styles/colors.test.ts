import { readFileSync, readdirSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

/** The library's checkerboard and the canvas's safe-area shade draw from the colour tokens, never raw colours. */
const appCss = readFileSync(resolve(__dirname, 'app.css'), 'utf8');
const rule = (selector: string) => new RegExp(`${selector.replace('.', '\\.')} \\{([^}]*)\\}`).exec(appCss)![1];

const SRC = resolve(__dirname, '..');

/** Every style sheet of the page but the tokens themselves. */
function sheets(dir = SRC): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sheets(path);
    return entry.name.endsWith('.css') && entry.name !== 'tokens.css' ? [path] : [];
  });
}

/** Each innermost rule of a sheet, comments left out: its selector and its declarations. */
function rules(path: string): { file: string; selector: string; body: string }[] {
  const css = readFileSync(path, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
  return [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => ({
    file: relative(SRC, path),
    selector: m[1].trim(),
    body: m[2],
  }));
}

const ALL = sheets().flatMap(rules);

describe('colours', () => {
  it('the checkerboard and the safe-area shade use tokens', () => {
    for (const selector of ['.checker', '.safe-area-shapes']) {
      const body = rule(selector);
      expect(body, selector).toMatch(/var\(--color-/);
      expect(body, selector).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(/i);
    }
  });

  it('the interface draws only from the tokens, so it follows the theme', () => {
    for (const { file, selector, body } of ALL) {
      // The video is black in both themes; masks use colour only for their alpha.
      if (selector === '.frame.is-socket') continue;
      const painted = body.replace(/(-webkit-)?mask-image:[^;]*;/g, '');
      expect(painted, `${file} ${selector}`).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(|\b(white|black)\s*;/i);
    }
  });

  it('navy is only ever text on yellow; text and edges on paper, red and lane colours use their own ink', () => {
    const navy = ALL.filter(({ body }) => body.includes('--color-navy'));
    expect(navy.length).toBeGreaterThan(0);
    for (const { file, selector, body } of navy) {
      expect(body, `${file} ${selector}`).toMatch(/--color-hilite/);
    }
  });

  it('what a proposal takes away is striped red, and the time range you chose is a plain wash', () => {
    const striped = /repeating-linear-gradient\(135deg, var\(--color-cut-hatch\)/;
    expect(rule('.tl-cut'), '.tl-cut').toMatch(striped);
    expect(rule('.tl-ghost'), '.tl-ghost').toMatch(striped);
    expect(rule('.tl-range'), '.tl-range').not.toMatch(/repeating-linear-gradient|--color-cut/);
  });

  it('the video and what is drawn over it stay dark in both themes', () => {
    // The layers' outlines, handles, snap guides and safe area over the preview; a caption style's sample and the
    // backdrop it is shown on (the frame's own ring, when the style is chosen, follows the panel); the Record dialog's
    // live preview, black behind what it shows and what is said over it, and the camera bubble.
    for (const selector of [
      '.frame-handles',
      '.style-frame::before',
      '.style-frame > *',
      '.record-preview',
      '.record-bubble',
    ]) {
      const found = ALL.filter((r) => r.selector.split(',').some((s) => s.trim() === selector));
      expect(found.length, selector).toBeGreaterThan(0);
      expect(
        found.some(({ body }) => /color-scheme:\s*dark;/.test(body)),
        selector,
      ).toBe(true);
    }
  });
});
