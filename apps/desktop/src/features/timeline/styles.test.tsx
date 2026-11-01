import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { compile } from 'tailwindcss';
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { longProject } from '../../test/longProject';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');

/** The app's own stylesheet, as written (jsdom draws no CSS at all, so a class with no rule goes unnoticed there). */
const appCss = readFileSync(resolve(__dirname, '../../styles/app.css'), 'utf8');

/** Lucide's own icon classes, which need no rule. */
const ICONS = /^lucide(-|$)/;

/** Whether app.css has a rule for `.name`: a class with no rule draws nothing, which no jsdom count would notice. */
const styled = (name: string) => new RegExp(`\\.${escape(name)}(?![\\w-])`).test(appCss);
const escape = (name: string) => name.replace(/[^\w-]/g, '\\$&');

/**
 * The classes Tailwind 4 really makes a utility of, from the page's own Tailwind entry (`styles/tailwind.css`: the
 * theme, the utilities and `theme-bridge.css`'s colours), compiled for `names`. A colour utility only exists for a colour
 * `theme-bridge.css` maps, which is what `bg-clip-deep` missed.
 */
async function tailwindUtilities(names: string[]): Promise<Set<string>> {
  const styles = resolve(__dirname, '../../styles');
  const tailwind = resolve(__dirname, '../../../node_modules/tailwindcss');
  const compiler = await compile(readFileSync(resolve(styles, 'tailwind.css'), 'utf8'), {
    base: styles,
    loadStylesheet: async (id, base) => {
      const path = id.startsWith('tailwindcss/')
        ? resolve(tailwind, id.slice('tailwindcss/'.length))
        : resolve(base, id);
      return { path, base: dirname(path), content: readFileSync(path, 'utf8') };
    },
  });
  const css = compiler.build(names);
  return new Set(names.filter((name) => hasClass(css, name)));
}

/** Whether `css` has a selector for the class `name` by its whole name: `.flex-no` is not in `.flex-none`. */
function hasClass(css: string, name: string): boolean {
  const selector = `.${escape(name)}`;
  for (let at = css.indexOf(selector); at !== -1; at = css.indexOf(selector, at + 1)) {
    // What follows is no more of a class name (a letter, a digit, `-`, `_`, or an escape's `\`).
    if (!/[\w\\-]/.test(css.charAt(at + selector.length))) return true;
  }
  return false;
}

/** Every class on the elements `selector` finds that neither app.css nor Tailwind defines. */
async function unstyled(selector: string): Promise<string[]> {
  const names = [...new Set([...document.querySelectorAll(selector)].flatMap((el) => [...el.classList]))].filter(
    (name) => !ICONS.test(name) && !styled(name),
  );
  const utilities = await tailwindUtilities(names);
  return names.filter((name) => !utilities.has(name));
}

const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
beforeAll(() =>
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 1240 }),
);
afterAll(() => width && Object.defineProperty(HTMLElement.prototype, 'clientWidth', width));

beforeEach(() => {
  useSelection.setState({ selectedId: null });
});

describe('the timeline draws with classes app.css defines', () => {
  it("counts a Tailwind class only by its whole name, never as the start of another's", async () => {
    expect(await tailwindUtilities(['flex-none', 'flex-no'])).toEqual(new Set(['flex-none']));
  });

  it('zoomed out, every lane draws its bands in a colour app.css gives them', async () => {
    useProject.setState({ snapshot: { ...fixtureSnapshot(), project: longProject() }, pending: 0 });
    useUi.setState({ pxPerSecond: 0.2 });
    renderWithProviders(<Timeline />);
    const bands = document.querySelectorAll('[data-band]');
    expect(bands.length).toBeGreaterThan(0);
    expect(document.querySelector('[data-lane="main"] [data-band]')).toHaveClass('tl-band', 'is-video');
    expect(await unstyled('[data-band]')).toEqual([]);
  });

  it('clips, layers and their parts carry only styled classes', async () => {
    useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
    useUi.setState({ pxPerSecond: 40 });
    useSelection.setState({ selectedId: 'a' });
    renderWithProviders(<Timeline />);
    expect(document.querySelectorAll('[data-block-id]').length).toBeGreaterThan(2);
    expect(await unstyled('[data-block-id], [data-block-id] *, [data-lane], [data-lane] > *')).toEqual([]);
  });

  it('the small controls in the gutter and on the Video lane reach out with a pad that takes presses', () => {
    // Marker flags, lane buttons and track toggles are under the design's 28 px: each has an invisible pad.
    const pads = ['.tl-marker::before', '.icon-btn.tl-lane-btn::before', '.icon-btn.tl-track-toggle::before'];
    for (const selector of pads) {
      const rule = new RegExp(`${selector.replace(/[.]/g, '\\.')}[^{]*\\{([^}]*)\\}`).exec(appCss);
      expect(rule, selector).not.toBeNull();
      expect(rule![1], selector).toMatch(/inset:/);
    }
  });
});

