import { act, fireEvent, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Snapshot } from '../../bindings/Snapshot';
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
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { useToasts } = await import('../../state/toasts');
const { useEditFrames } = await import('../../state/editFrames');
const { Timeline } = await import('./Timeline');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');

const S = 1_000_000;
const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));
const settle = () => act(() => new Promise((r) => setTimeout(r, 450)));
const block = (name: RegExp) => screen.getByRole('button', { name });

function Editor() {
  useEditorShortcuts();
  return <Timeline />;
}

/** "a" plays the recording 2 to 20 s, "b" broll 2 to 8 s (2 s spare each side), "c" the recording 30 to 40 s. */
function abc(): Snapshot {
  const snapshot = fixtureSnapshot();
  const [a, b] = snapshot.project.timeline.main;
  snapshot.project.timeline.main = [
    { ...a, sourceInUs: 2 * S },
    { ...b, sourceInUs: 2 * S, sourceOutUs: 8 * S },
    { ...a, id: 'c', sourceInUs: 30 * S, sourceOutUs: 40 * S },
  ];
  return snapshot;
}

beforeEach(() => {
  useProject.setState({ snapshot: abc(), pending: 0 });
  useSelection.setState({ selectedId: null, selectedIds: [] });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: false });
  useToasts.setState({ toasts: [] });
  vi.mocked(engine.seek).mockClear();
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

describe('rolling a cut', () => {
  it('dragging the cut moves both clips’ edges together and commits one Roll', async () => {
    renderWithProviders(<Editor />);
    // The cut after a is at 18 s (720 px at 40 px/s).
    const handle = document.querySelector<HTMLElement>('[data-roll="a"]')!;
    expect(handle).toHaveAttribute('data-tip', 'Drag to roll this cut');
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 720, clientY: 10 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 760, clientY: 10 });
    await frames();
    const [a, b] = [block(/^Recording 1, clip 1/), block(/^broll\.mp4/)];
    expect(a.style.width).toBe('760px');
    expect(b.style.left).toBe('760px');
    expect(b.style.width).toBe('200px');
    expect(screen.getByText('Roll +1.0 s')).toBeInTheDocument();
    // The preview shows the frame at the new cut, the playhead left where it was.
    expect(vi.mocked(engine.seek).mock.calls.at(-1)?.[0]).toBe(19 * S);
    expect(usePlayback.getState().playheadUs).toBe(0);
    // Beside it, the two frames that meet at the new cut, with the change.
    expect(useEditFrames.getState().change).toBe('Roll +1.0 s');
    expect(useEditFrames.getState().pair).toEqual([
      { path: '/media/screen', fileUs: 21 * S - 33_333 },
      { path: '/media/broll', fileUs: 3 * S },
    ]);
    fireEvent.pointerUp(handle, { pointerId: 1, clientX: 760, clientY: 10 });
    await frames();
    expect(useEditFrames.getState().pair).toBeNull();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.roll.undo',
      ops: [{ type: 'roll', clipId: 'a', byUs: S }],
    });
  });

  it('a roll stops where the footage ends and says which clip ran out', async () => {
    renderWithProviders(<Editor />);
    const handle = document.querySelector<HTMLElement>('[data-roll="a"]')!;
    // 3 s earlier: b has only 2 s before its in point.
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 720, clientY: 10 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 600, clientY: 10 });
    await frames();
    expect(screen.getByText('Roll −2.0 s · broll.mp4 has no more footage that way')).toBeInTheDocument();
    fireEvent.pointerUp(handle, { pointerId: 1, clientX: 600, clientY: 10 });
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.roll.undo',
      ops: [{ type: 'roll', clipId: 'a', byUs: -2 * S }],
    });
  });

  it('⌥. rolls the cut nearest the playhead a frame; held, the presses are one Roll', async () => {
    usePlayback.setState({ playheadUs: 17 * S });
    renderWithProviders(<Editor />);
    for (let i = 0; i < 3; i++) fireEvent.keyDown(window, { key: '≥', code: 'Period', altKey: true });
    expect(bridge.apply).not.toHaveBeenCalled();
    await settle();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.roll.undo',
      ops: [{ type: 'roll', clipId: 'a', byUs: 100_000 }],
    });
  });
});

describe('slipping and sliding a clip', () => {
  it('⌥-dragging a clip slips its footage: earlier footage to the right, its place and length kept', async () => {
    renderWithProviders(<Editor />);
    const b = block(/^broll\.mp4/);
    // b is 18 to 24 s (720 to 960 px): grabbed in its middle, dragged 1 s right.
    fireEvent.pointerDown(b, { pointerId: 1, button: 0, clientX: 840, clientY: 10, altKey: true });
    fireEvent.pointerMove(b, { pointerId: 1, clientX: 880, clientY: 10, altKey: true });
    await frames();
    expect(b.style.getPropertyValue('--trim-dx')).toBe('-40px');
    expect(screen.getByText('Slip −1.0 s')).toBeInTheDocument();
    // The preview shows the clip's new first frame.
    expect(vi.mocked(engine.seek).mock.calls.at(-1)?.[0]).toBe(18 * S);
    fireEvent.pointerUp(b, { pointerId: 1, clientX: 880, clientY: 10 });
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.slip.undo',
      ops: [{ type: 'slip', clipId: 'b', byUs: -S }],
    });
    expect(useSelection.getState().selectedId).toBe('b');
  });

  it('⌥⇧-dragging a clip slides it between its neighbours, which trim to make room', async () => {
    renderWithProviders(<Editor />);
    const b = block(/^broll\.mp4/);
    fireEvent.pointerDown(b, { pointerId: 1, button: 0, clientX: 840, clientY: 10, altKey: true, shiftKey: true });
    fireEvent.pointerMove(b, { pointerId: 1, clientX: 800, clientY: 10, altKey: true, shiftKey: true });
    await frames();
    const [a, c] = [block(/^Recording 1, clip 1/), block(/^Recording 1, clip 3/)];
    expect(b.style.transform).toContain('-40px');
    expect(a.style.width).toBe('680px');
    expect(c.style.left).toBe('920px');
    expect(screen.getByText('Slide −1.0 s')).toBeInTheDocument();
    fireEvent.pointerUp(b, { pointerId: 1, clientX: 800, clientY: 10 });
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.slide.undo',
      ops: [{ type: 'slide', clipId: 'b', byUs: -S }],
    });
  });

  it('⌥⇧-dragging the first clip says a slide needs a clip each side, and changes nothing', async () => {
    renderWithProviders(<Editor />);
    const a = block(/^Recording 1, clip 1/);
    fireEvent.pointerDown(a, { pointerId: 1, button: 0, clientX: 300, clientY: 10, altKey: true, shiftKey: true });
    fireEvent.pointerMove(a, { pointerId: 1, clientX: 340, clientY: 10, altKey: true, shiftKey: true });
    await frames();
    expect(screen.getByText('Slide 0.0 s · Only a clip with a clip on each side can slide')).toBeInTheDocument();
    fireEvent.pointerUp(a, { pointerId: 1, clientX: 340, clientY: 10 });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it('⌥⇧, slips the clip under the playhead a frame earlier, and says when there is no more footage', async () => {
    usePlayback.setState({ playheadUs: 20 * S });
    renderWithProviders(<Editor />);
    fireEvent.keyDown(window, { key: '¯', code: 'Comma', altKey: true, shiftKey: true });
    await settle();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.slip.undo',
      ops: [{ type: 'slip', clipId: 'b', byUs: -33_333 }],
    });
    // The first clip has no footage before its in point once slipped to it.
    vi.mocked(bridge.apply).mockClear();
    const snapshot = abc();
    snapshot.project.timeline.main[0].sourceInUs = 0;
    act(() => useProject.setState({ snapshot }));
    usePlayback.setState({ playheadUs: 5 * S });
    fireEvent.keyDown(window, { key: '¯', code: 'Comma', altKey: true, shiftKey: true });
    await settle();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(useToasts.getState().toasts.at(-1)?.text).toBe('There is no more footage that way.');
  });
});
