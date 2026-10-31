import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

/** The library's checkerboard and the canvas's safe-area shade draw from the colour tokens, never raw colours. */
const appCss = readFileSync(resolve(__dirname, 'app.css'), 'utf8');
const rule = (selector: string) => new RegExp(`${selector.replace('.', '\\.')} \\{([^}]*)\\}`).exec(appCss)![1];

describe('colours', () => {
  it('the checkerboard and the safe-area shade use tokens', () => {
    for (const selector of ['.checker', '.safe-area-shapes']) {
      const body = rule(selector);
      expect(body, selector).toMatch(/var\(--color-/);
      expect(body, selector).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(/i);
    }
  });
});
