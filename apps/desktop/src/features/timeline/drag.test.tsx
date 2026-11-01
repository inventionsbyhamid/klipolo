import { act, fireEvent, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { builtinId } from '../../lib/contrib';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { bridge } = await import('../../bridge/commands');
const { engine } = await import('../../bridge/engine');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { LiveRegion } = await import('./LiveRegion');
const { announce } = await import('./announce');

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));
const block = (name: RegExp) => screen.getByRole('button', { name });

async function drag(el: HTMLElement, fromX: number, toX: number, init: Partial<PointerEventInit> = {}) {
  fireEvent.pointerDown(el, { pointerId: 1, button: 0, clientX: fromX, clientY: 10 });
  fireEvent.pointerMove(el, { pointerId: 1, clientX: toX, clientY: 10, ...init });
  await frames();
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  usePlayback.setState({ playheadUs: 25_000_000, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: true });
  vi.mocked(bridge.apply)
    .mockReset()
    .mockImplementation(async () => {
      const s = useProject.getState().snapshot!;
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { clips: [], items: [] },
      };
    });
  vi.mocked(engine.previewClear).mockClear();
});

describe('dragging blocks', () => {
  it('dragging a layer moves it and commits one moveItem on release', async () => {
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 140); // +40 px at 40 px/s: +1 s
    expect(title.style.transform).toContain('40px');
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames(); // `useProject.apply` sends from its queue, a microtask later
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.move.layer.undo',
      ops: [{ type: 'moveItem', id: 'title', trackId: null, startUs: 2_000_000 }],
    });
  });

  it('a layer snaps to the playhead within 8 px, unless ⌘ is held', async () => {
    usePlayback.setState({ playheadUs: 2_100_000 });
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 140);
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    expect(vi.mocked(bridge.apply).mock.calls[0][0].ops[0]).toMatchObject({ startUs: 2_100_000 });
    await drag(block(/Set up billing/), 100, 140, { metaKey: true });
    fireEvent.pointerUp(block(/Set up billing/), { pointerId: 1, clientX: 140, clientY: 10, metaKey: true });
    await frames();
    expect(vi.mocked(bridge.apply).mock.calls[1][0].ops[0]).toMatchObject({ startUs: 2_000_000 });
  });

  it('a snap shows its guide across the lanes and in the ruler row, which the sticky ruler covers', async () => {
    usePlayback.setState({ playheadUs: 2_100_000 });
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 140);
    const guides = [...document.querySelectorAll<HTMLElement>('[data-snap-guide]')];
    expect(guides.map((g) => g.dataset.snapGuide)).toEqual(['ruler', 'lanes']);
    expect(guides[0].closest('.tl-head')).not.toBeNull();
    for (const guide of guides) {
      expect(guide).toHaveStyle({ display: 'block', transform: 'translateX(84px)' }); // 2.1 s at 40 px/s
    }
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    for (const guide of guides) expect(guide.style.display).toBe('');
  });

  it("dragging a clip past the next clip's middle commits moveClip", async () => {
    renderWithProviders(<Timeline />);
    const a = block(/^Recording 1/);
    // Pressed at 2.5 s; b's middle is at 25 s: 27.5 s passes it.
    await drag(a, 100, 1100);
    fireEvent.pointerUp(a, { pointerId: 1, clientX: 1100, clientY: 10 });
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.move.clip.undo',
      ops: [{ type: 'moveClip', id: 'a', index: 1 }],
    });
  });

  it('attached layers follow a dragged clip on screen', async () => {
    renderWithProviders(<Timeline />);
    await drag(block(/^Recording 1/), 100, 300);
    expect(block(/Set up billing/).style.transform).toContain('200px');
  });

  it('escape cancels a drag and clears the dry run', async () => {
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 160);
    fireEvent.keyDown(window, { key: 'Escape' });
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 160, clientY: 10 });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(engine.previewClear).toHaveBeenCalled();
    expect(title.style.transform).toBe('');
  });

  it('a new revision mid-drag cancels it', async () => {
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 160);
    const s = useProject.getState().snapshot!;
    act(() =>
      useProject.setState({
        snapshot: { ...s, history: { ...s.history, revision: s.history.revision + 1 } },
      }),
    );
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 160, clientY: 10 });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(title.style.transform).toBe('');
  });

  it('pointercancel reverts without a commit', async () => {
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 160);
    fireEvent.pointerCancel(title, { pointerId: 1 });
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(title.style.transform).toBe('');
  });
  it('a dragged block stays mounted, moving and focused when autoscroll takes its origin out of the window', async () => {
    renderWithProviders(<Timeline />);
    const scroller = screen.getByTestId('ruler').closest<HTMLElement>('.overflow-auto')!;
    vi.spyOn(scroller, 'getBoundingClientRect').mockReturnValue({
      left: 0,
      right: 1200,
      width: 1200,
    } as DOMRect);
    const title = block(/Set up billing/);
    // Held 8 px from the lanes' right edge: autoscroll runs. The press focused the block.
    await drag(title, 100, 1192);
    expect(scroller.scrollLeft).toBeGreaterThan(0);
    // Far on: the window (from 23 s) no longer holds the title's origin (1 to 4 s).
    act(() => {
      scroller.scrollLeft = 2000;
      scroller.dispatchEvent(new Event('scroll'));
    });
    await frames();
    expect(document.querySelector('[data-block-id="title"]')).toBe(title);
    // It follows the pointer and the scroll (to the frame grid: 1.33 px at 40 px/s and 30 fps).
    const dx = Number(/translate3d\(([\d.]+)px/.exec(title.style.transform)?.[1]);
    expect(Math.abs(dx - (1192 - 100 + scroller.scrollLeft))).toBeLessThan(1.4);
    expect(document.activeElement).toBe(title);
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 1192, clientY: 10 });
    await frames();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
  });

  it('a layer nudged 3 px moves 3 px with snapping on: the playhead its press moved is no magnet', async () => {
    // With no intro, a press puts the playhead (at 25 s) on the title's own start.
    const snapshot = fixtureSnapshot();
    const track = snapshot.project.timeline.tracks[0];
    const [title] = track.items;
    track.items = [{ ...title, layout: { ...title.layout, motion: undefined } }];
    useProject.setState({ snapshot });
    renderWithProviders(<Timeline />);
    const el = block(/Set up billing/);
    await drag(el, 100, 103);
    expect(usePlayback.getState().playheadUs).toBe(1_000_000);
    fireEvent.pointerUp(el, { pointerId: 1, clientX: 103, clientY: 10 });
    await frames();
    // 75 ms, on the 30 fps grid.
    expect(vi.mocked(bridge.apply).mock.calls[0][0].ops).toEqual([
      { type: 'moveItem', id: 'title', trackId: null, startUs: 1_066_667 },
    ]);
  });

  it('a layer already resting on the playhead still nudges 3 px with snapping on', async () => {
    // An earlier click parked the playhead on the title's start (1 s); its own origin is never a magnet.
    usePlayback.setState({ playheadUs: 1_000_000 });
    const snapshot = fixtureSnapshot();
    const track = snapshot.project.timeline.tracks[0];
    const [title] = track.items;
    track.items = [{ ...title, layout: { ...title.layout, motion: undefined } }];
    useProject.setState({ snapshot });
    renderWithProviders(<Timeline />);
    const el = block(/Set up billing/);
    await drag(el, 100, 103);
    fireEvent.pointerUp(el, { pointerId: 1, clientX: 103, clientY: 10 });
    await frames();
    expect(vi.mocked(bridge.apply).mock.calls[0][0].ops).toEqual([
      { type: 'moveItem', id: 'title', trackId: null, startUs: 1_066_667 },
    ]);
  });

  it('a block held still sends no new dry run', async () => {
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 140);
    // Let the drag's own frames settle first (a loaded machine can leave one pending).
    await frames();
    const sent = vi.mocked(engine.preview).mock.calls.length;
    expect(sent).toBeGreaterThan(0);
    for (const y of [11, 12]) {
      fireEvent.pointerMove(title, { pointerId: 1, clientX: 140, clientY: y });
      await frames();
    }
    expect(engine.preview).toHaveBeenCalledTimes(sent);
  });

  it('a release the block never hears (it lost the capture) still ends the drag', async () => {
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 140);
    fireEvent.pointerUp(window, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(title.dataset.dragging).toBeUndefined();
  });

  it('snapping turned off mid-drag (N) lets go of the snap at once', async () => {
    usePlayback.setState({ playheadUs: 2_100_000 });
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 140);
    expect(title.style.transform).toContain('44px'); // held to the playhead at 2.1 s
    act(() => useUi.getState().setSnapping(false));
    await frames();
    expect(title.style.transform).toContain('40px');
  });

  it('a reorder that leaves a transition no room says so', async () => {
    // a has a 1 s transition into b, which has 2 s of footage before it. Moved after b, a is last and loses it.
    const snapshot = fixtureSnapshot();
    const [a, b] = snapshot.project.timeline.main;
    const transitionOut = { effect: builtinId('transition.dissolve'), durationUs: 1_000_000, params: {} };
    snapshot.project.timeline.main = [
      { ...a, transitionOut },
      { ...b, sourceInUs: 2_000_000 },
    ];
    useProject.setState({ snapshot });
    renderWithProviders(<Timeline />);
    const clip = block(/^Recording 1/);
    await drag(clip, 100, 1100);
    expect(screen.getByText('Position 2 of 2 · A transition is removed.')).toBeInTheDocument();
    fireEvent.pointerUp(clip, { pointerId: 1, clientX: 1100, clientY: 10 });
    await frames();
    expect(screen.getByRole('status')).toHaveTextContent(
      'Moved Recording 1 to position 2. A transition is removed.',
    );
  });

  it('the live region stays one element; each message is new in it, so the same words twice are heard twice', () => {
    renderWithProviders(<LiveRegion />);
    const region = screen.getByRole('status');
    act(() => announce('Moved Recording 1 to position 2'));
    const first = region.firstElementChild;
    act(() => announce('Moved Recording 1 to position 2'));
    expect(screen.getByRole('status')).toBe(region);
    expect(region.firstElementChild).not.toBe(first);
    expect(region).toHaveTextContent('Moved Recording 1 to position 2');
  });
});
