import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
const { useProject } = await import('../../state/project');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');

const appCss = readFileSync(resolve(__dirname, '../../styles/app.css'), 'utf8');
const tokens = readFileSync(resolve(__dirname, '../../styles/tokens.css'), 'utf8');
const px = (pattern: RegExp, text = appCss) => Number(pattern.exec(text)![1]);

/**
 * A 1100 x 700 window is a short one: the timeline panel is `--timeline-h` there (196 px). What its lanes need, as
 * app.css lays them out in a short window: 8 px padding above and below, the overview row (6 px and 2 px each side),
 * a 4 px gap before the lanes' scroller, then the ruler and four lanes with 4 px between them.
 */
const SHORT = {
  panel: px(/max-height: 819px\)[^}]*--timeline-h: (\d+)px/s, tokens),
  padding: 2 * 8,
  overview: 6 + 2 * 2,
  gap: 4,
  ruler: 24,
};
const videoMin = px(/\.tl-row\.is-grow \{[^}]*min-height: (\d+)px/s);

/** The fixture with `rows` overlay rows: as many overlapping titles, each on a track of its own. */
function overlayRows(rows: number) {
  const snapshot = fixtureSnapshot();
  const [overlay] = snapshot.project.timeline.tracks;
  const [title] = overlay.items;
  for (let n = 1; n < rows; n++)
    snapshot.project.timeline.tracks.push({
      ...overlay,
      id: `o${n}`,
      items: [{ ...title, id: `title${n}`, attach: null } as Item],
    });
  return snapshot;
}

/** The heights the lanes render with (inline), and the Video lane's spare height in the short timeline. */
function laneHeights() {
  const height = (lane: string) =>
    parseFloat(document.querySelector<HTMLElement>(`[data-lane="${lane}"]`)!.style.height);
  const fixed = { layers: height('layers'), sounds: height('sounds'), music: height('music') };
  const { panel, padding, overview, gap, ruler } = SHORT;
  const video = panel - padding - overview - gap - ruler - 4 * gap - fixed.layers - fixed.sounds - fixed.music;
  return { ...fixed, video };
}

const matchMedia = window.matchMedia;
beforeEach(() => {
  useUi.setState({ pxPerSecond: 40 });
  // A short window: under 820 px tall.
  window.matchMedia = (query: string) =>
    ({ ...matchMedia(query), matches: query.includes('max-height: 819px') }) as MediaQueryList;
});
afterEach(() => {
  window.matchMedia = matchMedia;
});

describe('the lanes in a 1100 x 700 window', () => {
  it('with two overlay rows every lane fits: the Video lane gives way', () => {
    useProject.setState({ snapshot: overlayRows(2), pending: 0 });
    renderWithProviders(<Timeline />);
    const lanes = laneHeights();
    expect(lanes).toMatchObject({ layers: 44, sounds: 20, music: 28 });
    // About 34 px of Video, under its usual 44 and over its floor: nothing is clipped.
    expect(lanes.video).toBeLessThan(44);
    expect(lanes.video).toBeGreaterThanOrEqual(videoMin);
  });

  it('with three overlay rows the lanes scroll, so the Music lane is still reachable', () => {
    useProject.setState({ snapshot: overlayRows(3), pending: 0 });
    renderWithProviders(<Timeline />);
    const lanes = laneHeights();
    expect(lanes).toMatchObject({ layers: 64, sounds: 20, music: 28 });
    // The Video lane would need to go under its floor: the lanes' scroller scrolls instead of clipping.
    expect(lanes.video).toBeLessThan(videoMin);
    expect(appCss).toMatch(/\.tl-scroll \{[^}]*overflow-y: auto/s);
    // The ruler (and the playhead's head on it) stays at the top while they scroll, over the lanes passing under it.
    expect(appCss).toMatch(/\.tl-head \{[^}]*position: sticky;[^}]*top: 0;[^}]*background: var\(--color-bay\)/s);
  });
});
