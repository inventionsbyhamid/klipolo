import { act, fireEvent, screen, within } from '@testing-library/react';
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

  it("the group's card mutes every sound of it in one step", async () => {
    useSelection.getState().selectMany(['a', 'song']);
    renderWithProviders(<Editor />);
    const card = screen.getByRole('complementary', { name: '2 items settings' });
    expect(card).toHaveTextContent('Volume');
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
