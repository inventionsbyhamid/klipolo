import { act, fireEvent, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import { builtinId } from '../../lib/contrib';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
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
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { useToasts } = await import('../../state/toasts');
const { Timeline } = await import('./Timeline');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));
const block = (name: RegExp) => screen.getByRole('button', { name });
const edge = (el: HTMLElement, which: 'start' | 'end') =>
  el.querySelector<HTMLElement>(`[data-trim="${which}"]`)!;

function ShortcutsHarness() {
  useEditorShortcuts();
  return null;
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
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

describe('trimming and splitting', () => {
  it("dragging a clip's end edge trims it and commits one op", async () => {
    renderWithProviders(<Timeline />);
    const b = screen.getByRole('button', { name: /^broll\.mp4/ });
    const handle = b.querySelector('[data-trim="end"]')!;
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 1199, clientY: 10 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 1159, clientY: 10 });
    await frames();
    expect(b.dataset.trimming).toBe('true');
    fireEvent.pointerUp(handle, { pointerId: 1, clientX: 1159, clientY: 10 });
    await frames(); // `useProject.apply` sends from its queue, a microtask later
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.trim.clip.undo',
      ops: [{ type: 'trimClip', id: 'b', edge: 'end', sourceUs: 9_000_000 }],
    });
  });

  it("a clip's end trim ripples the clips after it live; Escape puts everything back", async () => {
    renderWithProviders(<Timeline />);
    const a = block(/^screen\.mp4/);
    const b = block(/^broll\.mp4/);
    // −40 px at 40 px/s: a ends at 19 s, and b follows it 1 s earlier.
    fireEvent.pointerDown(edge(a, 'end'), { pointerId: 1, button: 0, clientX: 799, clientY: 10 });
    fireEvent.pointerMove(a, { pointerId: 1, clientX: 759, clientY: 10 });
    await frames();
    expect(a.style.width).toBe('760px');
    expect(b.style.transform).toContain('-40px');
    expect(screen.getByText('0:19.0, −1.0 s')).toBeInTheDocument();
    fireEvent.keyDown(window, { key: 'Escape' });
    fireEvent.pointerUp(a, { pointerId: 1, clientX: 759, clientY: 10 });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(a.style.width).toBe('800px');
    expect(a.dataset.trimming).toBeUndefined();
    expect(b.style.transform).toBe('');
  });

  it("a clip's end trimmed shorter slides the free layers after it too, music aside; a longer one leaves them", async () => {
    const S = 1_000_000;
    const snapshot = fixtureSnapshot();
    const [overlays, audio] = snapshot.project.timeline.tracks;
    // After clip a's end (20 s): a free card at 22 s and a free whoosh at 21 s. The song from 25 s ducks, so it is
    // music, which stays where it is through a cut (its refit fits it to the new end).
    overlays.items.push({ ...titleItem, id: 'card', startUs: 22 * S, durationUs: S, attach: null });
    const [song] = audio.items;
    audio.items = [
      { ...song, id: 'whoosh', startUs: 21 * S, durationUs: 2 * S },
      { ...song, startUs: 25 * S, durationUs: 5 * S, content: { ...song.content, duck: -12 } as Item['content'] },
    ];
    useProject.setState({ snapshot, pending: 0 });
    renderWithProviders(<Timeline />);
    const a = block(/^screen\.mp4/);
    const byId = (id: string) => document.querySelector<HTMLElement>(`[data-block-id="${id}"]`)!;
    // −40 px at 40 px/s: a ends at 19 s, and what follows it comes 1 s earlier.
    fireEvent.pointerDown(edge(a, 'end'), { pointerId: 1, button: 0, clientX: 799, clientY: 10 });
    fireEvent.pointerMove(a, { pointerId: 1, clientX: 759, clientY: 10 });
    await frames();
    expect(byId('card').style.transform).toContain('-40px');
    expect(byId('whoosh').style.transform).toContain('-40px');
    expect(byId('song').style.transform).toBe('');
    // +40 px: the clips after it move on, but a longer clip takes nothing away, so free layers stay.
    fireEvent.pointerMove(a, { pointerId: 1, clientX: 839, clientY: 10 });
    await frames();
    expect(block(/^broll\.mp4/).style.transform).toContain('40px');
    expect(byId('card').style.transform).toBe('');
    expect(byId('whoosh').style.transform).toBe('');
    fireEvent.keyDown(window, { key: 'Escape' });
    fireEvent.pointerUp(a, { pointerId: 1, clientX: 839, clientY: 10 });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it("a layer's edge trims to the frame, shows its new length and change, and commits one trimItem", async () => {
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    // The title is 1 to 4 s (40 to 160 px): its end goes 1 s later.
    fireEvent.pointerDown(edge(title, 'end'), { pointerId: 1, button: 0, clientX: 159, clientY: 10 });
    fireEvent.pointerMove(title, { pointerId: 1, clientX: 199, clientY: 10 });
    await frames();
    expect(title.style.width).toBe('160px');
    expect(screen.getByText('0:04.0, +1.0 s')).toBeInTheDocument();
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 199, clientY: 10 });
    await frames();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.trim.layer.undo',
      ops: [{ type: 'trimItem', id: 'title', edge: 'end', atUs: 5_000_000 }],
    });
    // An edge's press selects the layer but leaves the playhead where it was (a trim to it is the usual one).
    expect(useSelection.getState().selectedId).toBe('title');
    expect(usePlayback.getState().playheadUs).toBe(25_000_000);
  });

  it('S splits the selected layer at the playhead, else the clip', async () => {
    useSelection.setState({ selectedId: 'title' });
    usePlayback.setState({ playheadUs: 2_000_000 });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    fireEvent.keyDown(window, { key: 's' });
    await frames();
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'undo.split.layer',
      ops: [{ type: 'splitItem', id: 'title', atUs: 2_000_000 }],
    });
    // The playhead outside the selected title: S says why, and never splits the clip under it instead.
    act(() => usePlayback.setState({ playheadUs: 10_000_000 }));
    fireEvent.keyDown(window, { key: 's' });
    await frames();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(useToasts.getState().toasts.at(-1)?.text).toBe("The playhead isn't inside the selected title.");
    // Nothing selected: the clip under the playhead.
    act(() => useSelection.setState({ selectedId: null }));
    fireEvent.keyDown(window, { key: 's' });
    await frames();
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'undo.split.clip',
      ops: [{ type: 'split', atUs: 10_000_000 }],
    });
  });

  it("a layer's edge on the playhead trims by 3 px: it never snaps back to where it began", async () => {
    usePlayback.setState({ playheadUs: 1_000_000 });
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    fireEvent.pointerDown(edge(title, 'start'), { pointerId: 1, button: 0, clientX: 40, clientY: 10 });
    fireEvent.pointerMove(title, { pointerId: 1, clientX: 43, clientY: 10 });
    await frames();
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 43, clientY: 10 });
    await frames();
    // 75 ms, on the 30 fps grid.
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.trim.layer.undo',
      ops: [{ type: 'trimItem', id: 'title', edge: 'start', atUs: 1_066_667 }],
    });
  });

  it('a trim that leaves a transition no footage says so', async () => {
    // a (0 to 20 s of a 60 s recording) has a 1 s transition into b; its end trimmed out to the file's end leaves the
    // transition nothing to draw from.
    const snapshot = fixtureSnapshot();
    const [a, b] = snapshot.project.timeline.main;
    const transitionOut = { effect: builtinId('transition.dissolve'), durationUs: 1_000_000, params: {} };
    snapshot.project.timeline.main = [
      { ...a, transitionOut },
      { ...b, sourceInUs: 2_000_000 },
    ];
    useProject.setState({ snapshot });
    renderWithProviders(<Timeline />);
    const clip = block(/^screen\.mp4/);
    fireEvent.pointerDown(edge(clip, 'end'), { pointerId: 1, button: 0, clientX: 799, clientY: 10 });
    fireEvent.pointerMove(clip, { pointerId: 1, clientX: 2399, clientY: 10 });
    await frames();
    expect(screen.getByText('1:00.0, +40.0 s · A transition is removed.')).toBeInTheDocument();
  });

  it("a trim's dry run follows the edge: only an unchanged batch is skipped", async () => {
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    fireEvent.pointerDown(edge(title, 'end'), { pointerId: 1, button: 0, clientX: 159, clientY: 10 });
    for (const x of [199, 239]) {
      fireEvent.pointerMove(title, { pointerId: 1, clientX: x, clientY: 10 });
      await frames();
    }
    // The dry run goes out a frame after the drag's frame: waited for, not timed (a loaded machine is slower).
    await vi.waitFor(() =>
      expect(vi.mocked(engine.preview).mock.calls.at(-1)?.[2]).toEqual([
        { type: 'trimItem', id: 'title', edge: 'end', atUs: 6_000_000 },
      ]),
    );
    fireEvent.keyDown(window, { key: 'Escape' });
  });
});
