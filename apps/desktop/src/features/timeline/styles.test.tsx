import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
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

/**
 * Tailwind utilities the timeline's blocks carry besides app.css's classes, each checked to exist in Tailwind 4 (a
 * colour utility only exists for a colour `theme-bridge.css` maps), and lucide's own icon classes, which need no rule.
 */
const UTILITIES = new Set(['isolate', 'truncate', 'pointer-events-none', 'absolute', 'top-0', 'origin-top-left']);
const ICONS = /^lucide(-|$)/;

/** Whether app.css has a rule for `.name`: a class with no rule draws nothing, which no jsdom count would notice. */
const styled = (name: string) => new RegExp(`\\.${name.replace(/[-/[\]]/g, '\\$&')}(?![\\w-])`).test(appCss);

/** Every class on the elements `selector` finds that neither app.css nor the allowed utilities define. */
function unstyled(selector: string): string[] {
  const names = [...document.querySelectorAll(selector)].flatMap((el) => [...el.classList]);
  return [...new Set(names)].filter((name) => !UTILITIES.has(name) && !ICONS.test(name) && !styled(name));
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
  it('zoomed out, every lane draws its bands in a colour app.css gives them', () => {
    useProject.setState({ snapshot: { ...fixtureSnapshot(), project: longProject() }, pending: 0 });
    useUi.setState({ pxPerSecond: 0.2 });
    renderWithProviders(<Timeline />);
    const bands = document.querySelectorAll('[data-band]');
    expect(bands.length).toBeGreaterThan(0);
    expect(document.querySelector('[data-lane="main"] [data-band]')).toHaveClass('tl-band', 'is-video');
    expect(unstyled('[data-band]')).toEqual([]);
  });

  it('clips, layers and their parts carry only styled classes', () => {
    useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
    useUi.setState({ pxPerSecond: 40 });
    useSelection.setState({ selectedId: 'a' });
    renderWithProviders(<Timeline />);
    expect(document.querySelectorAll('[data-block-id]').length).toBeGreaterThan(2);
    expect(unstyled('[data-block-id], [data-block-id] *, [data-lane], [data-lane] > *')).toEqual([]);
  });
});
