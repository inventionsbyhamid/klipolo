import { act, fireEvent, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProjectPatch } from '../../bindings/ProjectPatch';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
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
const { useSelection } = await import('../../state/selection');
const { useToasts } = await import('../../state/toasts');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { useAnnouncer } = await import('./announce');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));
/** Past the 400 ms a burst of presses takes to settle into one commit. */
const settle = () => act(() => new Promise((r) => setTimeout(r, 450)));
const ops = (call: number) => vi.mocked(bridge.apply).mock.calls[call][0].ops;
const lastToast = () => useToasts.getState().toasts.at(-1)?.text;
const key = (k: ',' | '.', repeat = false) =>
  fireEvent.keyDown(window, { key: k, code: k === ',' ? 'Comma' : 'Period', repeat });

function ShortcutsHarness() {
  useEditorShortcuts();
  return null;
}

function renderTimeline() {
  renderWithProviders(
    <>
      <Timeline />
      <ShortcutsHarness />
    </>,
  );
}

/** The next commit stays in flight until `land(patch)`. A test that stops early still lets it land (`afterEach`). */
const holding: (() => void)[] = [];
function holdNextCommit() {
  const held = { land: (patch: Partial<ProjectPatch>) => void patch };
  holding.push(() => held.land({}));
  vi.mocked(bridge.apply).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        held.land = (patch) => {
          const s = useProject.getState().snapshot!;
          resolve({
            baseRevision: s.history.revision,
            history: { ...s.history, revision: s.history.revision + 1 },
            patch: { clips: [], items: [], ...patch },
          });
        };
      }),
  );
  return held;
}

/** The engine's answer to an apply or an undo: the next revision, nothing in the project changed. */
async function nextRevision() {
  const s = useProject.getState().snapshot!;
  return {
    baseRevision: s.history.revision,
    history: { ...s.history, revision: s.history.revision + 1 },
    patch: { clips: [], items: [] },
  };
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  useToasts.setState({ toasts: [] });
  usePlayback.setState({ playheadUs: 25_000_000, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: true });
  vi.mocked(bridge.apply).mockReset().mockImplementation(nextRevision);
  vi.mocked(bridge.undo).mockReset().mockImplementation(nextRevision);
});

afterEach(async () => {
  for (const land of holding.splice(0)) land();
  await settle();
});

// Drags and keyboard edits share one commit queue: each builds on the project the one before it left.
describe('keyboard edits and drags, one commit at a time', () => {
  it("a nudge pressed while a drop's commit is in flight builds on where the drop put the layer", async () => {
    const drop = holdNextCommit();
    renderTimeline();
    const title = screen.getByRole('button', { name: /Set up billing/ });
    fireEvent.pointerDown(title, { pointerId: 1, button: 0, clientX: 100, clientY: 10 });
    fireEvent.pointerMove(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    key('.');
    await act(async () => drop.land({ items: [{ ...titleItem, startUs: 2_000_000 }] }));
    await settle();
    expect(ops(0)).toEqual([{ type: 'moveItem', id: 'title', trackId: null, startUs: 2_000_000 }]);
    // One frame after where the drop put it, not after where it was.
    expect(ops(1)).toEqual([{ type: 'moveItem', id: 'title', trackId: null, startUs: 2_033_333 }]);
  });

  it("a drag pressed while a nudge's commit is in flight lifts once it lands, and builds on it", async () => {
    useSelection.setState({ selectedId: 'title' });
    const nudge = holdNextCommit();
    renderTimeline();
    key('.');
    const title = screen.getByRole('button', { name: /Set up billing/ });
    // The press lets the nudge go in; nothing is drawn against the project it is about to replace.
    fireEvent.pointerDown(title, { pointerId: 1, button: 0, clientX: 100, clientY: 10 });
    fireEvent.pointerMove(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    expect(title.dataset.dragging).toBeUndefined();
    await act(async () => nudge.land({ items: [{ ...titleItem, startUs: 1_033_333 }] }));
    await frames();
    expect(title.dataset.dragging).toBe('true');
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    expect(ops(0)).toEqual([{ type: 'moveItem', id: 'title', trackId: null, startUs: 1_033_333 }]);
    expect(ops(1)).toEqual([{ type: 'moveItem', id: 'title', trackId: null, startUs: 2_033_333 }]);
  });
});

// A press that waited behind a commit leaves a burst; whatever waited after it builds on that burst.
describe('edits that waited behind a commit, in order', () => {
  /** The title (1 to 4 s) selected, the playhead at 2 s inside it, and its end trim (W) in flight until `land`. */
  function trimInFlight() {
    useSelection.setState({ selectedId: 'title' });
    usePlayback.setState({ playheadUs: 2_000_000 });
    useUi.setState({ snapping: false });
    const trim = holdNextCommit();
    renderTimeline();
    fireEvent.keyDown(window, { key: 'w' });
    return trim;
  }
  const kinds = () => vi.mocked(bridge.apply).mock.calls.map(([tx]) => tx.ops[0].type);
  /** What went to the engine, in order: each apply's first op, or `undo`. */
  const sent = () => {
    const applies = vi.mocked(bridge.apply).mock;
    const undos = vi.mocked(bridge.undo).mock.invocationCallOrder.map((at) => [at, 'undo'] as const);
    return [
      ...applies.calls.map(([tx], i) => [applies.invocationCallOrder[i], tx.ops[0].type] as const),
      ...undos,
    ]
      .sort(([a], [b]) => a - b)
      .map(([, what]) => what);
  };

  it('. then Q: the nudge goes in before the trim that waited after it', async () => {
    const trim = trimInFlight();
    await frames();
    key('.');
    fireEvent.keyDown(window, { key: 'q' });
    await act(async () => trim.land({}));
    await settle();
    expect(kinds()).toEqual(['trimItem', 'moveItem', 'trimItem']);
    expect(ops(1)).toEqual([{ type: 'moveItem', id: 'title', trackId: null, startUs: 1_033_333 }]);
    expect(ops(2)).toEqual([{ type: 'trimItem', id: 'title', edge: 'start', atUs: 2_000_000 }]);
  });

  it('., Q then W: each goes in in the order pressed, the third never overtaking the second', async () => {
    const trim = trimInFlight();
    await frames();
    key('.');
    fireEvent.keyDown(window, { key: 'q' });
    fireEvent.keyDown(window, { key: 'w' });
    await act(async () => trim.land({}));
    await settle();
    expect(kinds()).toEqual(['trimItem', 'moveItem', 'trimItem', 'trimItem']);
    expect(ops(2)).toMatchObject([{ type: 'trimItem', edge: 'start' }]);
    expect(ops(3)).toMatchObject([{ type: 'trimItem', edge: 'end' }]);
  });

  it('., ⌘Z then Q: the undo waits for the nudge pressed before it and takes it back; Q comes after', async () => {
    const trim = trimInFlight();
    await frames();
    key('.');
    fireEvent.keyDown(window, { key: 'z', metaKey: true });
    fireEvent.keyDown(window, { key: 'q' });
    await act(async () => trim.land({}));
    await settle();
    expect(sent()).toEqual(['trimItem', 'moveItem', 'undo', 'trimItem']);
    expect(ops(2)).toMatchObject([{ type: 'trimItem', edge: 'start' }]);
  });

  it('. then a drop: the nudge goes in first, and the drop builds on it', async () => {
    const trim = trimInFlight();
    await frames();
    key('.');
    // The engine puts the nudged title a frame later.
    vi.mocked(bridge.apply).mockImplementationOnce(async () => {
      const s = useProject.getState().snapshot!;
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { clips: [], items: [{ ...titleItem, startUs: 1_033_333 }] },
      };
    });
    const title = screen.getByRole('button', { name: /Set up billing/ });
    fireEvent.pointerDown(title, { pointerId: 1, button: 0, clientX: 100, clientY: 10 });
    fireEvent.pointerMove(title, { pointerId: 1, clientX: 140, clientY: 10 });
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await act(async () => trim.land({}));
    await settle();
    expect(kinds()).toEqual(['trimItem', 'moveItem', 'moveItem']);
    expect(ops(1)).toEqual([{ type: 'moveItem', id: 'title', trackId: null, startUs: 1_033_333 }]);
    // +1 s from where the nudge put it.
    expect(ops(2)).toEqual([{ type: 'moveItem', id: 'title', trackId: null, startUs: 2_033_333 }]);
  });

  it('a drag commit whose finishing work throws still lets the next edit through', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const off = useAnnouncer.subscribe(() => {
      off();
      throw new Error('The announcer broke.');
    });
    renderTimeline();
    const title = screen.getByRole('button', { name: /Set up billing/ });
    fireEvent.pointerDown(title, { pointerId: 1, button: 0, clientX: 100, clientY: 10 });
    fireEvent.pointerMove(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    key('.');
    await settle();
    expect(bridge.apply).toHaveBeenCalledTimes(2);
    expect(error).toHaveBeenCalled();
    error.mockRestore();
  });
});

describe('keys that can’t act', () => {
  it('edit keys do nothing while a drag is under way; the drop is the one edit', async () => {
    useSelection.setState({ selectedId: 'title' });
    usePlayback.setState({ playheadUs: 2_000_000 });
    renderTimeline();
    fireEvent.keyDown(window, { key: 'c', metaKey: true });
    const title = screen.getByRole('button', { name: /Set up billing/ });
    fireEvent.pointerDown(title, { pointerId: 1, button: 0, clientX: 100, clientY: 10 });
    fireEvent.pointerMove(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await frames();
    for (const k of ['Delete', 'Backspace', 'q', 'w', 's']) fireEvent.keyDown(window, { key: k });
    fireEvent.keyDown(window, { key: 'v', metaKey: true });
    fireEvent.keyDown(window, { key: 'x', metaKey: true });
    fireEvent.keyDown(window, { key: 'd', metaKey: true });
    key('.');
    await settle();
    expect(bridge.apply).not.toHaveBeenCalled();
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 140, clientY: 10 });
    await settle();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(ops(0)).toEqual([{ type: 'moveItem', id: 'title', trackId: null, startUs: 2_000_000 }]);
  });

  it('a held , stops at 0:00, so a . straight after moves at once', async () => {
    // The title starts at 1 s: 30 frames.
    useSelection.setState({ selectedId: 'title' });
    renderTimeline();
    key(',');
    for (let i = 0; i < 39; i++) key(',', true);
    key('.');
    await settle();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(ops(0)).toEqual([{ type: 'moveItem', id: 'title', trackId: null, startUs: 33_333 }]);
  });

  it('Q, W, , and . say why they can’t, once per held key', async () => {
    // broll.mp4 is 20 to 30 s and the last clip; the playhead is at 5 s.
    useSelection.setState({ selectedId: 'b' });
    usePlayback.setState({ playheadUs: 5_000_000 });
    renderTimeline();
    fireEvent.keyDown(window, { key: 'q' });
    await frames();
    expect(lastToast()).toBe("The playhead isn't inside the selected clip.");
    key('.');
    await frames();
    expect(lastToast()).toBe('The clip is already last.');
    key('.', true);
    await frames();
    expect(useToasts.getState().toasts).toHaveLength(2);
    expect(bridge.apply).not.toHaveBeenCalled();
  });
});
