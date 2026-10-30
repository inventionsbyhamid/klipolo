import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

// The dialog's stylesheet, applied for real: the test runner blanks CSS imports (`?raw` too), so it is read from the
// disk. jsdom drops any declaration that uses `var()`, so the tokens become one plain color and the ring is checked by
// its shape: 2 px, inset.
const css = readFileSync(join(__dirname, 'settings.css'), 'utf8');
let sheet: HTMLStyleElement;
beforeEach(() => {
  sheet = document.createElement('style');
  // In front, the app's own focus outline (app.css's `:focus-visible` in its base layer), which the fields replace.
  const appOutline = ':focus { outline: 2px solid white; outline-offset: 2px; }\n';
  sheet.textContent = appOutline + css.replace(/var\(--[\w-]+\)/g, 'white');
  document.head.append(sheet);
});
afterEach(() => sheet.remove());

function fields() {
  render(
    <div className="settings-dialog">
      <input aria-label="Access code" className="input" />
      <input aria-label="Saved code" className="input" value="KLIP-TEST-0001" readOnly />
    </div>,
  );
}

describe('Settings fields', () => {
  it('show their own 2 px inset ring when focused, and no outline', () => {
    fields();
    for (const name of ['Access code', 'Saved code']) {
      const field = screen.getByLabelText(name);
      // jsdom keeps an element's computed style until its attributes change, not when focus moves: read the focused
      // style first, and touch an attribute before reading it again.
      field.focus();
      const focused = getComputedStyle(field);
      expect(focused.boxShadow, name).toMatch(/^inset 0 0 0 2px /);
      // jsdom keeps the `outline` shorthand whole (its longhands stay at their defaults), so the shorthand is read.
      expect(focused.outline, name).toBe('none');
      field.blur();
      field.dataset.blurred = 'yes';
      expect(getComputedStyle(field).boxShadow, name).toMatch(/^inset 0 0 0 1px /);
    }
  });
});
