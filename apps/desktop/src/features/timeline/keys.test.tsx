import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
    play: vi.fn().mockResolvedValue(undefined),
    pause: vi.fn().mockResolvedValue(0),
  },
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { useTimelineFocus } = await import('../../state/timelineFocus');
const { Timeline } = await import('./Timeline');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');
const { zoomTimeline } = await import('./zoomControl');

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));

function ShortcutsHarness() {
  useEditorShortcuts();
  return null;
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  useTimelineFocus.setState({ activeId: null, pending: false });
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
});

describe('the timeline from the keyboard', () => {
  it('arrow keys move focus along a lane, and selection follows', async () => {
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    // Anchored: the title's name ends "moves with screen.mp4".
    const a = screen.getByRole('button', { name: /^screen\.mp4/ });
    act(() => a.focus());
    // Straight to the next clip: Klipolo's cuts are no stops of their own (it draws no transitions).
    fireEvent.keyDown(a, { key: 'ArrowRight' });
    await frames();
    const b = screen.getByRole('button', { name: /^broll\.mp4/ });
    expect(b).toHaveFocus();
    expect(useSelection.getState().selectedId).toBe('b');
    expect(b.tabIndex).toBe(0);
    expect(a.tabIndex).toBe(-1);
  });

  it('ArrowDown goes to the lane below, at the same time', async () => {
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    const title = screen.getByRole('button', { name: /Set up billing/ });
    act(() => title.focus());
    fireEvent.keyDown(title, { key: 'ArrowDown' });
    await frames();
    expect(screen.getByRole('button', { name: /^screen\.mp4/ })).toHaveFocus();
  });

  it('holding . nudges a layer as one undo step', async () => {
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    for (let i = 0; i < 3; i++) fireEvent.keyDown(window, { key: '.', code: 'Period' });
    expect(bridge.apply).not.toHaveBeenCalled();
    await act(() => new Promise((r) => setTimeout(r, 450)));
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.move.layer.undo',
      ops: [{ type: 'moveItem', id: 'title', trackId: null, startUs: 1_100_000 }],
    });
  });

  it('pressing a block lets a settling nudge go in first, and the drag builds on it', async () => {
    useSelection.setState({ selectedId: 'title' });
    // The nudge lands: the title one frame later.
    vi.mocked(bridge.apply).mockImplementationOnce(async () => {
      const s = useProject.getState().snapshot!;
      const title = s.project.timeline.tracks[0].items[0];
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { clips: [], items: [{ ...title, startUs: 1_033_333 }] },
      };
    });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    fireEvent.keyDown(window, { key: '.', code: 'Period' });
    const title = screen.getByRole('button', { name: /Set up billing/ });
    fireEvent.pointerDown(title, { pointerId: 1, button: 0, clientX: 100, clientY: 10 });
    await frames(); // the nudge lands under the press
    fireEvent.pointerMove(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    const calls = vi.mocked(bridge.apply).mock.calls.map(([tx]) => tx.ops);
    expect(calls).toEqual([
      [{ type: 'moveItem', id: 'title', trackId: null, startUs: 1_033_333 }],
      [{ type: 'moveItem', id: 'title', trackId: null, startUs: 2_033_333 }],
    ]);
  });

  it('⌘C then ⌘V pastes a copy of the layer at the playhead', async () => {
    useSelection.setState({ selectedId: 'title' });
    usePlayback.setState({ playheadUs: 5_000_000 });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    fireEvent.keyDown(window, { key: 'c', metaKey: true });
    fireEvent.keyDown(window, { key: 'v', metaKey: true });
    await frames(); // `useProject.apply` sends from its queue, a microtask later
    const { label, ops } = vi.mocked(bridge.apply).mock.calls[0][0];
    expect(label).toBe('undo.paste');
    expect(ops[0]).toMatchObject({
      type: 'addItem',
      track: { type: 'auto' },
      attach: true,
      item: { startUs: 5_000_000, attach: null },
    });
    expect((ops[0] as { item: { id: string } }).item.id).not.toBe('title');
  });

  it('reaches blocks outside the drawn window and inside a band', async () => {
    // 400 px/s: the window ends near 5 s, so b (20 s) isn't drawn.
    useUi.setState({ pxPerSecond: 400 });
    const { unmount } = renderWithProviders(<Timeline />);
    expect(document.querySelector('[data-block-id="b"]')).toBeNull();
    const a = screen.getByRole('button', { name: /^screen\.mp4/ });
    act(() => a.focus());
    // b at 20 s is outside the window: focus on it draws it.
    fireEvent.keyDown(a, { key: 'ArrowRight' });
    await frames();
    expect(screen.getByRole('button', { name: /^broll\.mp4/ })).toHaveFocus();
    unmount();
    // 0.5 px/s: both clips are narrower than a block draws. a is where Tab enters (kept a block); b is in a band.
    useSelection.setState({ selectedId: null });
    useTimelineFocus.setState({ activeId: null, pending: false });
    useUi.setState({ pxPerSecond: 0.5 });
    renderWithProviders(<Timeline />);
    expect(document.querySelector('[data-block-id="b"]')).toBeNull();
    const first = screen.getByRole('button', { name: /^screen\.mp4/ });
    expect(first.tabIndex).toBe(0);
    act(() => first.focus());
    fireEvent.keyDown(first, { key: 'End' });
    await frames();
    expect(screen.getByRole('button', { name: /^broll\.mp4/ })).toHaveFocus();
  });

  it('a focused block that is deleted hands focus to its neighbour, never to the page', async () => {
    // The engine takes a out: b is the only clip left.
    vi.mocked(bridge.apply).mockImplementation(async () => {
      const s = useProject.getState().snapshot!;
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { main: ['b'], clips: [], items: [] },
      };
    });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    const b = screen.getByRole('button', { name: /^broll\.mp4/ });
    act(() => b.focus());
    // Over the cut between them, to a.
    fireEvent.keyDown(b, { key: 'ArrowLeft' });
    await frames();
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowLeft' });
    await frames();
    const a = screen.getByRole('button', { name: /^screen\.mp4/ });
    expect(a).toHaveFocus();
    // Chromium and WebView2 blur a focused block React moves (a clip's nudge reorders them): focus is back on it by
    // the time the blur is looked at, so it still counts as the timeline's.
    fireEvent.focusOut(a);
    await act(async () => undefined);
    fireEvent.keyDown(a, { key: 'Delete' });
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.delete.clip',
      ops: [{ type: 'removeClip', id: 'a' }],
    });
    expect(a.isConnected).toBe(false);
    expect(document.activeElement).toBe(screen.getByRole('button', { name: /^broll\.mp4/ }));
    // The selection follows, so a second ⌫ deletes the neighbour.
    expect(useSelection.getState().selectedId).toBe('b');
    expect(screen.getByRole('status')).toHaveTextContent('Deleted screen.mp4');
  });

  it("W trims the selected clip's end to the playhead, and says so", async () => {
    useSelection.setState({ selectedId: 'b' });
    usePlayback.setState({ playheadUs: 28_000_000 });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    fireEvent.keyDown(window, { key: 'w' });
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.trim.clip.undo',
      ops: [{ type: 'trimClip', id: 'b', edge: 'end', sourceUs: 8_000_000 }],
    });
    expect(screen.getByRole('status')).toHaveTextContent('Trimmed broll.mp4 to 0:08.0');
  });

  /**
   * Three layers at 10 to 14 s fill compact view's three rows; the focused, selected title is nudged onto them and
   * packed into "+N". Returns the marker, which has focus.
   */
  async function packFocusedTitle() {
    const snapshot = fixtureSnapshot();
    const [overlay] = snapshot.project.timeline.tracks;
    const [title] = overlay.items;
    for (const n of [3, 4, 5])
      snapshot.project.timeline.tracks.push({
        ...overlay,
        id: `t${n}`,
        name: `Overlay ${n}`,
        items: [{ ...title, id: `o${n}`, startUs: 10_000_000, attach: null }],
      });
    useProject.setState({ snapshot });
    useSelection.setState({ selectedId: 'title' });
    // The engine puts the title at 10 s.
    vi.mocked(bridge.apply).mockImplementationOnce(async () => {
      const s = useProject.getState().snapshot!;
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { clips: [], items: [{ ...title, startUs: 10_000_000 }] },
      };
    });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    act(() => document.querySelector<HTMLElement>('[data-block-id="title"]')!.focus());
    fireEvent.keyDown(window, { key: '.', code: 'Period' });
    await act(() => new Promise((r) => setTimeout(r, 450)));
    expect(document.querySelector('[data-block-id="title"]')).toBeNull();
    return document.activeElement as HTMLElement;
  }
  const MARKER_NAME = '+1 more layer: Set up billing, selected';

  it('a layer packed into "+N" hands focus to the marker, which names it; ⌫ deletes it, Enter selects it', async () => {
    const marker = await packFocusedTitle();
    // Focus went to the "+N" marker that stands for it; the selection stayed on it.
    expect(marker).toHaveAccessibleName(MARKER_NAME);
    expect(useSelection.getState().selectedId).toBe('title');
    fireEvent.keyDown(marker, { key: 'Delete' });
    await frames();
    expect(vi.mocked(bridge.apply).mock.calls[1][0]).toEqual({
      label: 'undo.delete.layer',
      ops: [{ type: 'removeItem', id: 'title' }],
    });
    // Enter on the marker selects the layer it stands for (Klipolo shows no track rows); focus stays on the marker.
    await userEvent.keyboard('{Enter}');
    await frames();
    expect(document.activeElement).toBe(marker);
    expect(useSelection.getState().selectedId).toBe('title');
  });

  it('a zoom keeps focus on the "+N" marker, still standing for the selected layer', async () => {
    const marker = await packFocusedTitle();
    act(() => zoomTimeline(1.25));
    await frames();
    expect(useUi.getState().pxPerSecond).toBe(50);
    expect(document.activeElement).toBe(marker);
    expect(marker).toHaveAccessibleName(MARKER_NAME);
    expect(useSelection.getState().selectedId).toBe('title');
  });

  it('when its run scrolls out of the window, focus and the selection go to a block in view together', async () => {
    await packFocusedTitle();
    const scroller = screen.getByTestId('ruler').closest<HTMLElement>('.overflow-auto')!;
    // 100 s in: the window starts past the run at 10 to 14 s.
    act(() => {
      scroller.scrollLeft = 4000;
      scroller.dispatchEvent(new Event('scroll'));
    });
    await frames();
    expect(document.querySelector('[data-more-from]')).toBeNull();
    const focused = (document.activeElement as HTMLElement).dataset.blockId;
    expect(focused).toBeDefined();
    expect(useSelection.getState().selectedId).toBe(focused);
    // ⌫ deletes what has focus, not the layer out of sight.
    fireEvent.keyDown(document.activeElement!, { key: 'Delete' });
    await frames();
    expect(vi.mocked(bridge.apply).mock.calls[1][0].ops[0]).not.toMatchObject({ id: 'title' });
  });

  it('Tab off the "+N" marker onto a block brings the selection with it, so ⌫ deletes that block', async () => {
    const marker = await packFocusedTitle();
    expect(marker.dataset.moreFrom).toBeDefined();
    await userEvent.tab();
    const focused = (document.activeElement as HTMLElement).dataset.blockId;
    expect(focused).toBeDefined();
    expect(useSelection.getState().selectedId).toBe(focused);
    fireEvent.keyDown(document.activeElement!, { key: 'Delete' });
    await frames();
    expect(vi.mocked(bridge.apply).mock.calls[1][0].ops[0]).toMatchObject({ id: focused });
  });

  it('focus coming in while the selected layer is packed away selects the block it lands on, so ⌫ deletes that', async () => {
    // The title at 10 s, under three layers there: packed into "+N" (not drawn), and selected (picked on the preview).
    const snapshot = fixtureSnapshot();
    const [overlay] = snapshot.project.timeline.tracks;
    const [title] = overlay.items;
    overlay.items = [{ ...title, startUs: 10_000_000 }];
    for (const n of [3, 4, 5])
      snapshot.project.timeline.tracks.push({
        ...overlay,
        id: `t${n}`,
        name: `Overlay ${n}`,
        items: [{ ...title, id: `o${n}`, startUs: 10_000_000, attach: null }],
      });
    useProject.setState({ snapshot });
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    expect(document.querySelector('[data-block-id="title"]')).toBeNull();
    // Tab lands on the block in the Tab order: the clip in view.
    const stop = [...document.querySelectorAll<HTMLElement>('[data-block-id]')].find(
      (el) => el.tabIndex === 0,
    )!;
    expect(stop.dataset.blockId).toBe('a');
    act(() => stop.focus());
    expect(useSelection.getState().selectedId).toBe('a');
    fireEvent.keyDown(stop, { key: 'Delete' });
    await frames();
    expect(vi.mocked(bridge.apply).mock.calls[0][0]).toEqual({
      label: 'undo.delete.clip',
      ops: [{ type: 'removeClip', id: 'a' }],
    });
  });

  it('after focus leaves the timeline, a render never pulls it back to the "+N" marker', async () => {
    const marker = await packFocusedTitle();
    // A click on the preview (or a button, in WebKit) sends focus to the page.
    act(() => marker.blur());
    await act(async () => undefined);
    act(() => useSelection.getState().select('b'));
    await frames();
    expect(document.activeElement).toBe(document.body);
    expect(useSelection.getState().selectedId).toBe('b');
  });

  /**
   * The engine adds the next layer an edit makes at `startUs`: on the title's track, or (`lifted`) on a new track of
   * its own, as it does when the layer would overlap another.
   */
  function addsLayerAt(startUs: number, lifted = false) {
    vi.mocked(bridge.apply).mockImplementationOnce(async (tx) => {
      const s = useProject.getState().snapshot!;
      const op = tx.ops[0];
      if (op.type !== 'addItem') throw new Error('an edit that adds a layer');
      const copy = { ...op.item, startUs };
      const tracks = s.project.timeline.tracks.map(({ items, ...header }) => ({
        ...header,
        items: [...items.map((i) => i.id), ...(!lifted && header.id === 't1' ? [copy.id] : [])],
      }));
      if (lifted) tracks.push({ ...tracks[0], id: 't6', name: 'Overlay 6', items: [copy.id] });
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { clips: [], tracks, items: [copy] },
      };
    });
  }

  it.each([
    ['⌘D', 'd'],
    ['⌘V', 'v'],
  ])(
    '%s on the "+N" marker takes focus to the new layer, with the selection',
    async (_, key) => {
      const marker = await packFocusedTitle();
      if (key === 'v') fireEvent.keyDown(marker, { key: 'c', metaKey: true });
      // Where nothing else is: drawn in its own row, not packed.
      addsLayerAt(20_000_000);
      fireEvent.keyDown(marker, { key, metaKey: true });
      await frames();
      const tx = vi.mocked(bridge.apply).mock.calls[1][0];
      const added = tx.ops[0].type === 'addItem' ? tx.ops[0].item.id : '';
      expect(useSelection.getState().selectedId).toBe(added);
      expect(document.activeElement).toBe(document.querySelector(`[data-block-id="${added}"]`));
    },
  );

  it('⌘D on the "+N" marker of a layer whose copy is packed too: the marker stands for the copy', async () => {
    const marker = await packFocusedTitle();
    // Lifted to a track of its own over the four layers at 10 to 13 s: packed away like the title.
    addsLayerAt(11_000_000, true);
    fireEvent.keyDown(marker, { key: 'd', metaKey: true });
    await frames();
    const tx = vi.mocked(bridge.apply).mock.calls[1][0];
    const added = tx.ops[0].type === 'addItem' ? tx.ops[0].item.id : '';
    expect(useSelection.getState().selectedId).toBe(added);
    expect((document.activeElement as HTMLElement).dataset.moreFrom).toBeDefined();
    // The marker stands for the copy now: Enter selects the copy, not the original, and focus stays on the marker.
    await userEvent.keyboard('{Enter}');
    await frames();
    expect((document.activeElement as HTMLElement).dataset.moreFrom).toBeDefined();
    expect(useSelection.getState().selectedId).toBe(added);
  });

  it('←/→ on the "+N" marker of a packed layer go from where it is, and the selection follows', async () => {
    // The title (1 to 4 s) under three layers at 0.5 s: packed away. The clip under it is screen.mp4.
    const snapshot = fixtureSnapshot();
    const [overlay] = snapshot.project.timeline.tracks;
    const [title] = overlay.items;
    for (const n of [3, 4, 5])
      snapshot.project.timeline.tracks.push({
        ...overlay,
        id: `t${n}`,
        name: `Overlay ${n}`,
        items: [{ ...title, id: `o${n}`, startUs: 500_000, attach: null }],
      });
    snapshot.project.timeline.tracks[0].items = [{ ...title, startUs: 4_500_000 }];
    useProject.setState({ snapshot });
    useSelection.setState({ selectedId: 'title' });
    vi.mocked(bridge.apply).mockImplementationOnce(async () => {
      const s = useProject.getState().snapshot!;
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { clips: [], items: [{ ...title, startUs: 1_000_000 }] },
      };
    });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    act(() => document.querySelector<HTMLElement>('[data-block-id="title"]')!.focus());
    // A nudge earlier; the engine puts the title at 1 s, under the three.
    fireEvent.keyDown(window, { key: ',', code: 'Comma' });
    await act(() => new Promise((r) => setTimeout(r, 450)));
    const marker = document.activeElement as HTMLElement;
    expect(marker.dataset.moreFrom).toBeDefined();
    fireEvent.keyDown(marker, { key: 'ArrowLeft' });
    await frames();
    // The layer before it in the Layers lane (o5, the top track's, draws first at equal starts).
    expect((document.activeElement as HTMLElement).dataset.blockId).toMatch(/^o/);
    expect(useSelection.getState().selectedId).toBe((document.activeElement as HTMLElement).dataset.blockId);
  });
});
