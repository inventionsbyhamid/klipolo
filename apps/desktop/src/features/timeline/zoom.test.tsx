import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { fitPps, scrollForAnchor, zoomLimits } from './model/zoom';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({ engine: { seek: vi.fn(() => Promise.resolve()) } }));
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');
const { fitTimeline, zoomTimeline } = await import('./zoomControl');

describe('zoom', () => {
  it('zoom is clamped to fit and forty px per frame', () => {
    // An hour in 1000 px (minus a 24 px margin).
    expect(fitPps(3_600_000_000, 1000)).toBeCloseTo(976 / 3600, 6);
    expect(zoomLimits(3_600_000_000, 1000, 30)).toEqual([976 / 3600, 1200]);
    // A 10 s project never zooms out past 8 px/s.
    expect(zoomLimits(10_000_000, 1000, 60)[0]).toBe(8);
    expect(zoomLimits(10_000_000, 1000, 60)[1]).toBe(2400);
  });

  it('keeps the anchor under the same x', () => {
    // 30 s under x = 200 at 80 px/s: the lanes scroll to 2200 px.
    expect(scrollForAnchor(30_000_000, 200, 80)).toBe(2200);
    expect(scrollForAnchor(1_000_000, 200, 80)).toBe(0);
  });
});

// The scroller is 1240 px wide: 1200 px of lanes after the 40 px gutter. jsdom puts it at client x 0.
const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
beforeAll(() =>
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 1240 }),
);
afterAll(() => width && Object.defineProperty(HTMLElement.prototype, 'clientWidth', width));

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40 });
});

function Editor() {
  useEditorShortcuts();
  return (
    <div>
      <Timeline />
    </div>
  );
}

const scroller = () => screen.getByTestId('ruler').closest<HTMLElement>('.overflow-auto')!;
const nextFrame = () => act(() => new Promise<void>((done) => requestAnimationFrame(() => done())));
/** A WebKit GestureEvent (not in jsdom): `scale` since the gesture began, and the pointer when it carries one. */
const gesture = (type: string, init: { scale?: number; clientX?: number } = {}) =>
  fireEvent(scroller(), Object.assign(new Event(type, { cancelable: true }), init));
/** How many times the zoom changed. */
function countZooms() {
  const zooms = { count: 0 };
  const off = useUi.subscribe((s, prev) => {
    if (s.pxPerSecond !== prev.pxPerSecond) zooms.count++;
  });
  return { zooms, off };
}

describe('zooming the timeline', () => {
  it('a pinch (ctrl + wheel) zooms around the pointer, once per frame', async () => {
    renderWithProviders(<Editor />);
    const { zooms, off } = countZooms();
    // 400 px into the lanes at 40 px/s is 10 s. A half step, then two quarter steps in one frame: two zooms, ×2 in
    // all, and 10 s stays under the pointer (each step's render scrolls the lanes before the next step reads them).
    const pinch = (share: number) =>
      fireEvent.wheel(scroller(), { ctrlKey: true, deltaY: -100 * share * Math.log(2), clientX: 40 + 400 });
    pinch(0.5);
    await nextFrame();
    pinch(0.25);
    pinch(0.25);
    expect(zooms.count).toBe(1);
    await nextFrame();
    off();
    expect(zooms.count).toBe(2);
    expect(useUi.getState().pxPerSecond).toBeCloseTo(80, 6);
    expect(scroller().scrollLeft).toBeCloseTo(10 * 80 - 400, 3);
    // A plain wheel scrolls; it doesn't zoom.
    fireEvent.wheel(scroller(), { deltaY: -100, clientX: 440 });
    await nextFrame();
    expect(useUi.getState().pxPerSecond).toBeCloseTo(80, 6);
  });

  it('a WebKit pinch zooms once even if ctrl+wheel comes with it, and keeps the playhead when it has no pointer', async () => {
    usePlayback.setState({ playheadUs: 12_000_000 });
    renderWithProviders(<Editor />);
    gesture('gesturestart');
    gesture('gesturechange', { scale: 2, clientX: 40 + 400 });
    fireEvent.wheel(scroller(), { ctrlKey: true, deltaY: -100 * Math.log(2), clientX: 40 + 400 });
    await nextFrame();
    // ×2 (not ×4) around 10 s under the pointer.
    expect(useUi.getState().pxPerSecond).toBeCloseTo(80, 6);
    expect(scroller().scrollLeft).toBeCloseTo(400, 3);
    gesture('gestureend');
    // No pointer position: around the playhead (12 s, 560 px in), not a jump to the start.
    gesture('gesturestart');
    gesture('gesturechange', { scale: 1.5 });
    await nextFrame();
    expect(useUi.getState().pxPerSecond).toBeCloseTo(120, 6);
    expect(scroller().scrollLeft).toBeCloseTo(12 * 120 - 560, 3);
  });

  it('a pinch whose gestureend never comes holds ⌘ + wheel back only a moment after its last change', async () => {
    renderWithProviders(<Editor />);
    const now = vi.spyOn(performance, 'now').mockReturnValue(1_000);
    const wheel = () =>
      fireEvent.wheel(scroller(), { ctrlKey: true, deltaY: -100 * Math.log(2), clientX: 40 + 400 });
    gesture('gesturestart');
    gesture('gesturechange', { scale: 2, clientX: 40 + 400 });
    await nextFrame();
    expect(useUi.getState().pxPerSecond).toBeCloseTo(80, 6);
    // 100 ms on: part of the pinch, so it only stops the page zooming.
    now.mockReturnValue(1_100);
    wheel();
    await nextFrame();
    expect(useUi.getState().pxPerSecond).toBeCloseTo(80, 6);
    // 200 ms after the last change and still no gestureend: ⌘ + wheel zooms again.
    now.mockReturnValue(1_200);
    wheel();
    await nextFrame();
    now.mockRestore();
    expect(useUi.getState().pxPerSecond).toBeCloseTo(160, 6);
  });

  it('never zooms the wrong way: Zoom in past the closest zoom does nothing, and Fit stops at 40 px per frame', () => {
    // A zoom kept from a 60 fps project (2400 px/s) in this 30 fps one, whose closest is 1200.
    useUi.setState({ pxPerSecond: 2400 });
    const { snapshot } = useProject.getState();
    const [a] = snapshot!.project.timeline.main;
    renderWithProviders(<Editor />);
    act(() => zoomTimeline(1.25));
    expect(useUi.getState().pxPerSecond).toBe(2400);
    // Half a second fits 1176 px at 2352 px/s, past 40 px per frame. The title goes too: a layer past the main track
    // would make the video longer; the song never does.
    const { tracks } = snapshot!.project.timeline;
    act(() =>
      useProject.setState({
        snapshot: {
          ...snapshot!,
          project: {
            ...snapshot!.project,
            timeline: {
              main: [{ ...a, sourceOutUs: 500_000 }],
              tracks: tracks.filter((t) => t.kind !== 'overlay'),
            },
          },
        },
      }),
    );
    act(() => fitTimeline());
    expect(useUi.getState().pxPerSecond).toBe(1200);
  });

  it('draws a frame grid on the ruler at the closest zooms only', () => {
    useUi.setState({ pxPerSecond: 1200 });
    const { unmount } = renderWithProviders(<Editor />);
    // 30 fps at 1200 px/s: a line every 40 px.
    const grid = document.querySelector<HTMLElement>('[data-frame-grid]')!;
    expect(grid.style.backgroundImage).toContain('40px');
    unmount();
    useUi.setState({ pxPerSecond: 40 });
    renderWithProviders(<Editor />);
    expect(document.querySelector('[data-frame-grid]')).toBeNull();
  });

  it('⇧Z fits the whole video, and ⌘= then zooms around the playhead', async () => {
    usePlayback.setState({ playheadUs: 5_000_000 });
    useUi.setState({ pxPerSecond: 200 });
    renderWithProviders(<Editor />);
    scroller().scrollLeft = 300;
    await userEvent.keyboard('{Shift>}Z{/Shift}');
    // 30 s in 1200 px less the 24 px margin.
    expect(useUi.getState().pxPerSecond).toBeCloseTo(1176 / 30, 6);
    expect(scroller().scrollLeft).toBe(0);
    // The playhead (5 s) is 196 px in; after ⌘= it is still there.
    await userEvent.keyboard('{Meta>}={/Meta}');
    const pps = (1176 / 30) * 1.25;
    expect(useUi.getState().pxPerSecond).toBeCloseTo(pps, 6);
    expect(scroller().scrollLeft).toBeCloseTo(5 * pps - 196, 3);
  });
});
