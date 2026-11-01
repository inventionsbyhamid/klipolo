import { act, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixturePatch, fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn() },
  mediaUrl: (p: string) => p,
}));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useRange } = await import('../../state/range');
const { selectionOf, useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { useClipboard } = await import('../../state/clipboard');
const { itemsWord } = await import('./keys/groupActions');
const { Timeline } = await import('./Timeline');
const { Inspector } = await import('../inspector/Inspector');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');

const S = 1_000_000;
const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));
const block = (name: RegExp) => screen.getByRole('button', { name });
const selected = () => [...selectionOf(useSelection.getState())].sort();

function Editor() {
  useEditorShortcuts();
  return (
    <>
      <Timeline />
      <Inspector format="landscape" />
    </>
  );
}

/** `withSecond`, with "c" (the recording 30 to 40 s) after b: a 0 to 20 s, b 20 to 30 s, c 30 to 40 s. */
function withC() {
  const snapshot = withSecond();
  const [a] = snapshot.project.timeline.main;
  snapshot.project.timeline.main.push({ ...a, id: 'c', sourceInUs: 30 * S, sourceOutUs: 40 * S });
  return snapshot;
}

const settle = () => act(() => new Promise((r) => setTimeout(r, 450)));

/** The fixture with a second title at 6 s, free, on the title's track. */
function withSecond() {
  const snapshot = fixtureSnapshot();
  snapshot.project.timeline.tracks[0].items.push({
    ...titleItem,
    id: 'second',
    startUs: 6 * S,
    durationUs: S,
    attach: null,
    content: { ...titleItem.content, ...{ text: 'Second step' } } as typeof titleItem.content,
  });
  return snapshot;
}

beforeEach(() => {
  useProject.setState({ snapshot: withSecond(), pending: 0 });
  useSelection.setState({ selectedId: null, selectedIds: [] });
  useRange.setState({ range: null });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: false });
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

describe('selecting several clips and layers', () => {
  it('⌘-click adds a block or takes it out, ⇧-click adds a range on one track, Escape clears', async () => {
    renderWithProviders(<Editor />);
    fireEvent.pointerDown(block(/Set up billing/), { pointerId: 1, button: 0, clientX: 50, clientY: 5 });
    fireEvent.pointerUp(block(/Set up billing/), { pointerId: 1, clientX: 50, clientY: 5 });
    fireEvent.pointerDown(block(/^broll\.mp4/), { pointerId: 2, button: 0, clientX: 900, clientY: 5, metaKey: true });
    expect(selected()).toEqual(['b', 'title']);
    expect(screen.getByRole('complementary', { name: '2 items settings' })).toBeInTheDocument();
    fireEvent.pointerDown(block(/^broll\.mp4/), { pointerId: 3, button: 0, clientX: 900, clientY: 5, metaKey: true });
    expect(selected()).toEqual(['title']);
    // From the title, ⇧-click on the second title takes in the layers between them on that track.
    fireEvent.pointerDown(block(/Second step/), { pointerId: 4, button: 0, clientX: 250, clientY: 5, shiftKey: true });
    expect(selected()).toEqual(['second', 'title']);
    // Both draw selected.
    expect(block(/Set up billing/)).toHaveAttribute('aria-pressed', 'true');
    expect(block(/Second step/)).toHaveAttribute('aria-pressed', 'true');
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(selected()).toEqual([]);
  });

  it('a box over empty lane space selects what it touches; one that touches nothing selects time', () => {
    renderWithProviders(<Editor />);
    const at = (id: string, left: number, right: number) =>
      (document.querySelector<HTMLElement>(`[data-block-id="${id}"]`)!.getBoundingClientRect = () =>
        DOMRect.fromRect({ x: left, y: 0, width: right - left, height: 20 }));
    at('title', 40, 160);
    at('second', 240, 280);
    const lane = screen.getByRole('group', { name: 'Overlays' }).querySelector<HTMLElement>('.tl-lane')!;
    fireEvent.pointerDown(lane, { button: 0, clientX: 180, clientY: 2, pointerId: 1 });
    fireEvent.pointerMove(lane, { clientX: 300, clientY: 15, pointerId: 1 });
    expect(selected()).toEqual(['second']);
    fireEvent.pointerMove(lane, { clientX: 100, clientY: 15, pointerId: 1 });
    expect(selected()).toEqual(['title']);
    expect(useRange.getState().range).toBeNull();
    fireEvent.pointerUp(lane, { clientX: 100, clientY: 15, pointerId: 1 });
    // Between the two titles the box touches neither: that drag selects the time.
    fireEvent.pointerDown(lane, { button: 0, clientX: 170, clientY: 2, pointerId: 2 });
    fireEvent.pointerMove(lane, { clientX: 230, clientY: 15, pointerId: 2 });
    fireEvent.pointerUp(lane, { clientX: 230, clientY: 15, pointerId: 2 });
    expect(selected()).toEqual([]);
    expect(useRange.getState().range).not.toBeNull();
  });

  it('⌘A in the timeline selects every clip and layer', () => {
    renderWithProviders(<Editor />);
    const a = block(/^Recording 1/);
    act(() => a.focus());
    fireEvent.keyDown(a, { key: 'a', metaKey: true });
    expect(selected()).toEqual(['a', 'b', 'second', 'song', 'title']);
  });
});

describe("a group's edits", () => {
  it('Delete takes the group out in one step, labelled with how many', async () => {
    useSelection.getState().selectMany(['b', 'second']);
    renderWithProviders(<Editor />);
    fireEvent.keyDown(window, { key: 'Delete' });
    await frames();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.group.delete',
      params: { count: 2 },
      ops: [
        { type: 'removeItem', id: 'second' },
        { type: 'deleteRanges', ranges: [{ start: 20 * S, end: 30 * S }] },
      ],
    });
  });

  it('dragging a block of the group moves every layer of it, offsets kept', async () => {
    useSelection.getState().selectMany(['title', 'second']);
    renderWithProviders(<Editor />);
    const second = block(/Second step/);
    fireEvent.pointerDown(second, { pointerId: 1, button: 0, clientX: 250, clientY: 5 });
    fireEvent.pointerMove(second, { pointerId: 1, clientX: 290, clientY: 5 });
    await frames();
    expect(second.style.transform).toContain('40px');
    expect(block(/Set up billing/).style.transform).toContain('40px');
    fireEvent.pointerUp(second, { pointerId: 1, clientX: 290, clientY: 5 });
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.group.move',
      params: { count: 2 },
      ops: [
        { type: 'moveItem', id: 'second', trackId: null, startUs: 7 * S },
        { type: 'moveItem', id: 'title', trackId: null, startUs: 2 * S },
      ],
    });
    // A click on one of them (no drag) selects it alone.
    fireEvent.pointerDown(second, { pointerId: 2, button: 0, clientX: 250, clientY: 5 });
    fireEvent.pointerUp(second, { pointerId: 2, clientX: 250, clientY: 5 });
    expect(selected()).toEqual(['second']);
  });

  it('dragging a group with a clip moves the clip along the Video track and its layers as far', async () => {
    useSelection.getState().selectMany(['a', 'second']);
    renderWithProviders(<Editor />);
    const a = block(/^Recording 1/);
    // a (0 to 20 s) dragged 12 s later: it goes after b, 10 s later, and the free title with it.
    fireEvent.pointerDown(a, { pointerId: 1, button: 0, clientX: 400, clientY: 5 });
    fireEvent.pointerMove(a, { pointerId: 1, clientX: 880, clientY: 5 });
    await frames();
    expect(a.style.transform).toContain('400px');
    expect(block(/^broll\.mp4/).style.transform).toContain('-800px');
    expect(block(/Second step/).style.transform).toContain('400px');
    fireEvent.pointerUp(a, { pointerId: 1, clientX: 880, clientY: 5 });
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.group.move',
      params: { count: 2 },
      ops: [
        { type: 'moveClip', id: 'b', index: 0 },
        { type: 'moveItem', id: 'second', trackId: null, startUs: 16 * S },
      ],
    });
  });

  it('a group with clips apart moves them together only once it reaches a new place, and says so', async () => {
    useProject.setState({ snapshot: withC(), pending: 0 });
    useSelection.getState().selectMany(['a', 'c']);
    renderWithProviders(<Editor />);
    const a = block(/^Recording 1, clip 1/);
    // A few pixels: a would still go first, so nothing changes and no step is added.
    fireEvent.pointerDown(a, { pointerId: 1, button: 0, clientX: 400, clientY: 5 });
    fireEvent.pointerMove(a, { pointerId: 1, clientX: 410, clientY: 5 });
    await frames();
    fireEvent.pointerUp(a, { pointerId: 1, clientX: 410, clientY: 5 });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    // 15 s later: past b, so a and c go together after it.
    fireEvent.pointerDown(a, { pointerId: 2, button: 0, clientX: 400, clientY: 5 });
    fireEvent.pointerMove(a, { pointerId: 2, clientX: 1000, clientY: 5 });
    await frames();
    expect(screen.getByText('Moves 2 clips together')).toBeInTheDocument();
    fireEvent.pointerUp(a, { pointerId: 2, clientX: 1000, clientY: 5 });
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.group.move',
      params: { count: 2 },
      ops: [{ type: 'moveClip', id: 'b', index: 0 }],
    });
    expect(screen.getByRole('status')).toHaveTextContent('Moves 2 clips together');
  });

  it('held, the arrows nudge the group as one step: a frame each, ⇧ a second; the playhead stays', async () => {
    useSelection.getState().selectMany(['title', 'second']);
    renderWithProviders(<Editor />);
    fireEvent.keyDown(window, { key: 'ArrowRight' });
    fireEvent.keyDown(window, { key: 'ArrowRight', repeat: true });
    fireEvent.keyDown(window, { key: '.', code: 'Period', repeat: true });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    await settle();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'timeline.group.move',
      params: { count: 2 },
      ops: [
        { type: 'moveItem', id: 'second', trackId: null, startUs: 6 * S + 100_000 },
        { type: 'moveItem', id: 'title', trackId: null, startUs: S + 100_000 },
      ],
    });
    fireEvent.keyDown(window, { key: 'ArrowLeft', shiftKey: true });
    await settle();
    expect(bridge.apply).toHaveBeenLastCalledWith(
      expect.objectContaining({ ops: [expect.objectContaining({ id: 'title', startUs: 0 }), expect.anything()] }),
    );
    expect(usePlayback.getState().playheadUs).toBe(0);
  });

  it('⇧Enter adds the focused block or takes it out; with a group, the arrows move focus and keep the group', () => {
    useSelection.getState().selectMany(['title', 'second']);
    renderWithProviders(<Editor />);
    const b = block(/^broll\.mp4/);
    act(() => b.focus());
    fireEvent.keyDown(b, { key: 'Enter', shiftKey: true });
    expect(selected()).toEqual(['b', 'second', 'title']);
    fireEvent.keyDown(b, { key: 'ArrowLeft' });
    expect(selected()).toEqual(['b', 'second', 'title']);
    expect(document.activeElement).not.toBe(b);
    act(() => b.focus());
    fireEvent.keyDown(b, { key: 'Enter', shiftKey: true });
    expect(selected()).toEqual(['second', 'title']);
  });

  it('duplicate and paste put the clips in first, and a title on a copied clip lands on its copy', async () => {
    useSelection.getState().selectMany(['a', 'title']);
    renderWithProviders(<Editor />);
    // a is 0 to 20 s with the title 1 s into it: the copy goes right after a, the title's copy 1 s into it.
    fireEvent.keyDown(window, { key: 'd', metaKey: true });
    await frames();
    const [dup] = vi.mocked(bridge.apply).mock.calls.at(-1)!;
    expect(dup.ops.map((op) => op.type)).toEqual(['insertClip', 'addItem']);
    expect(dup.ops[0]).toMatchObject({ index: 1 });
    expect(dup.ops[1]).toMatchObject({ item: { startUs: 21 * S }, attach: true });
    // Copied, then pasted with the playhead in b (20 to 30 s): after b, from 30 s, the title at 31 s. (The copies the
    // duplicate selected aren't in this test's project, which the mocked Rust leaves as it is.)
    act(() => useSelection.getState().selectMany(['a', 'title']));
    fireEvent.keyDown(window, { key: 'c', metaKey: true });
    usePlayback.setState({ playheadUs: 25 * S });
    fireEvent.keyDown(window, { key: 'v', metaKey: true });
    await frames();
    const [pasted] = vi.mocked(bridge.apply).mock.calls.at(-1)!;
    expect(pasted.ops.map((op) => op.type)).toEqual(['insertClip', 'addItem']);
    expect(pasted.ops[0]).toMatchObject({ index: 2 });
    expect(pasted.ops[1]).toMatchObject({ item: { startUs: 31 * S }, attach: true });
    expect(useClipboard.getState().group).toHaveLength(2);
  });

  it('says how many in the words for one and for many', () => {
    expect(itemsWord(1)).toBe('1 item');
    expect(itemsWord(3)).toBe('3 items');
  });

  it("the group's card mutes every sound of it in one step", async () => {
    useSelection.getState().selectMany(['a', 'song']);
    renderWithProviders(<Editor />);
    const card = screen.getByRole('complementary', { name: '2 items settings' });
    expect(card).toHaveTextContent('Volume');
    // a plays at 100 % and the song at 40 %: the card says they differ.
    expect(within(card).getByText('Mixed')).toBeInTheDocument();
    await userEvent.click(within(card).getByRole('button', { name: 'More' }));
    for (const name of ['Copy', 'Cut', 'Paste', 'Duplicate', 'Delete'])
      expect(screen.getByRole('menuitem', { name })).toBeInTheDocument();
    await userEvent.keyboard('{Escape}');
    fireEvent.click(within(card).getByRole('button', { name: 'Mute' }));
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith(
      expect.objectContaining({ label: 'timeline.group.mute', params: { count: 2 } }),
    );
  });

  it('after an undo the group keeps the blocks still there', async () => {
    useSelection.getState().selectMany(['title', 'second']);
    renderWithProviders(<Editor />);
    const without = fixtureSnapshot();
    vi.mocked(bridge.undo).mockResolvedValue(fixturePatch(without));
    await act(() => useProject.getState().undo());
    await frames();
    expect(selected()).toEqual(['title']);
  });
});
