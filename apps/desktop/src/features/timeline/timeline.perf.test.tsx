import { act, fireEvent } from '@testing-library/react';
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { applyPatch } from '../../lib/patch';
import { renderCounts } from '../../lib/renderCount';
import { fixtureSnapshot } from '../../test/fixtures';
import { longProject, longProjectWithSound } from '../../test/longProject';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
const { useProject } = await import('../../state/project');
const { useUi } = await import('../../state/ui');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Timeline } = await import('./Timeline');
const { zoomTimeline } = await import('./zoomControl');
const { itemSpans } = await import('./model/index');
const { useProposal, noRemovals } = await import('../../state/proposal');

const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
beforeAll(() =>
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 1240 }),
);
afterAll(() => width && Object.defineProperty(HTMLElement.prototype, 'clientWidth', width));

beforeEach(() => {
  useProject.setState({ snapshot: { ...fixtureSnapshot(), project: longProject() }, pending: 0 });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40 });
  useSelection.setState({ selectedId: null });
  // A test that moves the playhead (the ⌘− case) doesn't scroll the ones after it on mount.
  usePlayback.setState({ playheadUs: 0, playing: false });
  renderCounts.clear();
});

/** At most this many blocks mounted at any zoom (the 16 px rule's peak is about 270). */
const MOUNT_BUDGET = 280;
const blocks = () => document.querySelectorAll('[data-block-id]').length;
const rendered = () => [...renderCounts.values()].reduce((a, b) => a + b, 0);
const scroller = () =>
  document.querySelector<HTMLElement>('[data-testid="ruler"]')!.closest<HTMLElement>('.overflow-auto')!;
const scrollTo = (px: number) =>
  act(() => {
    scroller().scrollLeft = px;
    scroller().dispatchEvent(new Event('scroll'));
  });

describe('a 1-hour, 300-layer project', () => {
  it('renders a bounded number of blocks for an hour', () => {
    renderWithProviders(<Timeline />);
    // 1200 px of lanes at 40 px/s is 30 s; at the start the window is 0 to 60 s: 25 clips and 5 titles.
    expect(blocks()).toBeGreaterThan(20);
    expect(blocks()).toBeLessThanOrEqual(150);
  });

  it('sub-pixel clips merge into a band', () => {
    // 0.2 px/s: the whole hour in view, clips 0.48 px and titles 1.6 px wide. Clip 5 is selected, so it stays a block.
    useUi.setState({ pxPerSecond: 0.2 });
    useSelection.setState({ selectedId: 'c5' });
    renderWithProviders(<Timeline />);
    expect(document.querySelectorAll('[data-band]').length).toBeGreaterThan(0);
    expect(blocks()).toBeLessThanOrEqual(150);
    for (const el of document.querySelectorAll<HTMLElement>('[data-block-id], [data-band]')) {
      expect(Number.isFinite(parseFloat(el.style.left))).toBe(true);
    }
    // Drawn 16 px wide, it stays over the band after it: its lane draws every band first.
    const kept = document.querySelector('[data-block-id="c5"]')!;
    for (const band of kept.parentElement!.querySelectorAll('[data-band]'))
      expect(band.compareDocumentPosition(kept) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('at fit zoom the Overlays lane draws a few bands, not one per title', () => {
    // Fit: the hour in 1176 px, titles 2.6 px wide and 9 px apart on each of three tracks.
    useUi.setState({ pxPerSecond: 1176 / 3600 });
    renderWithProviders(<Timeline />);
    expect(document.querySelectorAll('[data-band]').length).toBeLessThanOrEqual(6);
    expect(blocks()).toBeLessThanOrEqual(5);
  });

  it('mid zoom mounts a bounded number of blocks, and a zoom step renders only its new window', () => {
    // 1 px/s, half an hour in: every clip (2.4 px) and title (8 px) is narrower than its block draws, so they band.
    useUi.setState({ pxPerSecond: 1 });
    renderWithProviders(<Timeline />);
    scrollTo(1800);
    expect(blocks()).toBeLessThanOrEqual(150);
    // A ⌘+wheel step (×e) around 40:00: titles become 22 px blocks. The step's first render already draws the new
    // window (≈ 29:00 to 51:00, about 110 titles), not the old one at the new scale (≈ 250).
    renderCounts.clear();
    act(() => zoomTimeline(Math.E, { us: 2400 * 1e6, x: 600 }));
    expect(blocks()).toBeGreaterThan(50);
    expect(blocks()).toBeLessThanOrEqual(150);
    expect(rendered()).toBe(blocks());
    // The toolbar's step (×1.25 around the view's centre) re-renders at most what it mounts.
    renderCounts.clear();
    act(() => zoomTimeline(1.25));
    expect(blocks()).toBeLessThanOrEqual(150);
    expect(rendered()).toBeLessThanOrEqual(150);
  });

  it('zooming out from 40 px/s stays within the mounted-block budget of 280, through the 16 px peak', () => {
    // 10 minutes in, the playhead at 10:15 (600 px into the view), so each ⌘− keeps it there.
    usePlayback.setState({ playheadUs: 615_000_000, playing: false });
    renderWithProviders(<Timeline />);
    scrollTo(600 * 40);
    for (let step = 1; step <= 8; step++) {
      act(() => zoomTimeline(0.8));
      expect(blocks()).toBeLessThanOrEqual(MOUNT_BUDGET);
    }
    // Eight presses: 40 × 0.8⁸ ≈ 6.71 px/s, where every clip (2.4 s) first reaches the 16 px a clip block draws: the
    // rule's peak, about 225 clips and 45 titles in the three-view window.
    expect(useUi.getState().pxPerSecond).toBeCloseTo(40 * 0.8 ** 8, 6);
    expect(blocks()).toBeGreaterThan(200);
    expect(blocks()).toBeLessThanOrEqual(MOUNT_BUDGET);
  });

  it('an edit re-renders only the block it changed', () => {
    renderWithProviders(<Timeline />);
    renderCounts.clear();
    const snapshot = useProject.getState().snapshot!;
    const title = snapshot.project.timeline.tracks[0].items[0];
    act(() =>
      useProject.setState({
        snapshot: {
          ...snapshot,
          project: applyPatch(snapshot.project, {
            clips: [],
            items: [{ ...title, startUs: title.startUs + 500_000 }],
          })!,
        },
      }),
    );
    expect(renderCounts.get(title.id)).toBe(1);
    expect(rendered()).toBe(1);
  });

  it('a focus move from the keyboard re-renders only the two blocks it moved between', async () => {
    renderWithProviders(<Timeline />);
    const c5 = document.querySelector<HTMLElement>('[data-block-id="c5"]')!;
    act(() => c5.focus());
    renderCounts.clear();
    fireEvent.keyDown(c5, { key: 'ArrowRight' });
    await act(() => new Promise((r) => setTimeout(r, 40)));
    // c5 leaves the Tab order, c6 enters it and is selected. The lanes re-render (their kept set changed); no other
    // block does.
    expect(document.activeElement).toBe(document.querySelector('[data-block-id="c6"]'));
    expect(renderCounts.get('c5')).toBe(1);
    expect(renderCounts.get('c6')).toBe(1);
    expect(rendered()).toBe(2);
  });

  it("a track's settings change re-renders no block", () => {
    renderWithProviders(<Timeline />);
    renderCounts.clear();
    const snapshot = useProject.getState().snapshot!;
    const shapes = snapshot.project.timeline.tracks.map(({ items, ...header }) => ({
      ...header,
      items: items.map((i) => i.id),
    }));
    shapes[0] = { ...shapes[0], name: 'Titles', muted: true };
    act(() =>
      useProject.setState({
        snapshot: {
          ...snapshot,
          project: applyPatch(snapshot.project, { clips: [], items: [], tracks: shapes })!,
        },
      }),
    );
    expect(useProject.getState().snapshot!.project.timeline.tracks[0].name).toBe('Titles');
    expect(rendered()).toBe(0);
    // The track keeps its items array, so its span index is reused rather than rebuilt.
    const before = snapshot.project.timeline.tracks[0].items;
    const after = useProject.getState().snapshot!.project.timeline.tracks[0].items;
    expect(after).toBe(before);
    expect(itemSpans(after)).toBe(itemSpans(before));
  });

  it("the ruler draws its window's ticks, not the hour's", () => {
    renderWithProviders(<Timeline />);
    const ticks = () => [...document.querySelectorAll('[data-tick]')].map((t) => t.textContent);
    // 2 s ticks at 40 px/s: the hour has 1,800; the 0 to 60 s window has 31.
    expect(ticks()).toHaveLength(31);
    // Half an hour in: the window is 29:30 to 31:00, and the ruler draws it alone.
    scrollTo(1800 * 40);
    expect(ticks()).toContain('30:00');
    expect(ticks().length).toBeLessThanOrEqual(50);
    expect(ticks()).not.toContain('0:00');
  });

  it('scrolling re-renders the lanes only after half a view', () => {
    renderWithProviders(<Timeline />);
    const block = (id: string) => document.querySelector(`[data-block-id="${id}"]`);
    // The window is 0 to 60 s; the view is 1200 px (30 s). Under half a view (600 px) from the start keeps it, so
    // clip 25 (60 s) still isn't drawn.
    scrollTo(500);
    expect(block('c24')).not.toBeNull();
    expect(block('c25')).toBeNull();
    // At 1600 px (40 s) the window moves to 10 to 100 s: clip 41 (98.4 s) is in, clips 0 and 42 (100.8 s) are out.
    scrollTo(1600);
    expect(block('c41')).not.toBeNull();
    expect(block('c42')).toBeNull();
    expect(block('c0')).toBeNull();
  });

  it('a chat proposal removing half the hour and every title ghosts only the window, and redraws no block', () => {
    renderWithProviders(<Timeline />);
    const S = 1_000_000;
    // Every other 2.4 s cut's footage (750 ranges) and all 300 titles.
    const ranges = Array.from({ length: 750 }, (_, k) => ({ start: 4.8 * k * S, end: (4.8 * k + 2.4) * S }));
    const items = [0, 1, 2].flatMap((t) => Array.from({ length: 100 }, (_, i) => `t${t}-${i}`));
    const ghosts = (kind: string) => document.querySelectorAll(`[data-ghost="${kind}"]`).length;
    renderCounts.clear();
    act(() =>
      useProposal.getState().show({ id: 'p', baseRevision: 3, removals: { ...noRemovals(), ranges, items }, tags: [] }),
    );
    // The overlay draws over the lanes: no block renders again.
    expect(rendered()).toBe(0);
    // The 0 to 60 s window: 13 of the 750 ranges (0, 4.8 … 57.6 s) and 5 of the 300 titles.
    expect(ghosts('footage')).toBe(13);
    expect(ghosts('layer')).toBe(5);
    // Half an hour in (the window is 29:30 to 31:00): still only the window's.
    scrollTo(1800 * 40);
    expect(ghosts('footage')).toBeGreaterThan(10);
    expect(ghosts('footage')).toBeLessThanOrEqual(25);
    expect(ghosts('layer')).toBeLessThanOrEqual(10);
    act(() => useProposal.getState().show(null));
    expect(ghosts('footage') + ghosts('layer')).toBe(0);
  });

  it('an hour of sound effects and music draws only the window in the Sounds and Music lanes, banded zoomed out', () => {
    useProject.setState({ snapshot: { ...fixtureSnapshot(), project: longProjectWithSound() }, pending: 0 });
    renderWithProviders(<Timeline />);
    const inLane = (lane: string, what: string) =>
      document.querySelectorAll(`[data-lane="${lane}"] ${what}`).length;
    // The 0 to 60 s window: 5 effects (one every 12 s) and one piece of music, of 300 and 20.
    expect(inLane('sounds', '[data-block-id]')).toBe(5);
    expect(inLane('music', '[data-block-id]')).toBe(1);
    expect(blocks()).toBeLessThanOrEqual(150);
    // Half an hour in: still the window's alone.
    scrollTo(1800 * 40);
    expect(inLane('sounds', '[data-block-id]')).toBeLessThanOrEqual(10);
    expect(inLane('music', '[data-block-id]')).toBeLessThanOrEqual(2);
    // Fitted (the hour in 1176 px): each effect is 0.33 px, so they band; the 3-minute pieces (59 px) stay blocks.
    scrollTo(0);
    renderCounts.clear();
    act(() => useUi.setState({ pxPerSecond: 1176 / 3600 }));
    expect(inLane('sounds', '[data-block-id]')).toBe(0);
    expect(inLane('sounds', '[data-band]')).toBeGreaterThan(0);
    expect(inLane('sounds', '[data-band]')).toBeLessThanOrEqual(6);
    expect(inLane('music', '[data-block-id]')).toBe(20);
    expect(blocks()).toBeLessThanOrEqual(MOUNT_BUDGET);
  });
});
