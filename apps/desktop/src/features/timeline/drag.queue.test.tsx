import { act, fireEvent, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProjectPatch } from '../../bindings/ProjectPatch';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), closeProject: vi.fn().mockResolvedValue(undefined) },
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
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));
const block = (name: RegExp) => screen.getByRole('button', { name });
const byId = (id: string) => document.querySelector<HTMLElement>(`[data-block-id="${id}"]`)!;
const ops = (call: number) => vi.mocked(bridge.apply).mock.calls[call][0].ops;

/** `el` drawn from `left` (jsdom lays nothing out). */
const drawnAt = (el: HTMLElement, left: number) =>
  vi.spyOn(el, 'getBoundingClientRect').mockReturnValue({ left } as DOMRect);

async function drag(el: HTMLElement, fromX: number, toX: number, target: HTMLElement = el) {
  fireEvent.pointerDown(target, { pointerId: 1, button: 0, clientX: fromX, clientY: 10 });
  fireEvent.pointerMove(el, { pointerId: 1, clientX: toX, clientY: 10 });
  await frames();
  fireEvent.pointerUp(el, { pointerId: 1, clientX: toX, clientY: 10 });
}

/**
 * The next commit stays in flight until `land(patch)` (the engine applying it and its revision arriving), or `fail()`.
 */
const holding: (() => void)[] = [];
function holdNextCommit() {
  const held: { land: (patch: Partial<ProjectPatch>) => void; fail: () => void } = {
    land: () => undefined,
    fail: () => undefined,
  };
  // A test that stops early still lets it settle: `useProject` runs commands one after another.
  holding.push(() => held.land({}));
  vi.mocked(bridge.apply).mockImplementationOnce(
    () =>
      new Promise((resolve, reject) => {
        held.land = (patch) => {
          // Landed already, and the project has since closed.
          const s = useProject.getState().snapshot;
          if (!s) return;
          resolve({
            baseRevision: s.history.revision,
            history: { ...s.history, revision: s.history.revision + 1 },
            patch: { clips: [], items: [], ...patch },
          });
        };
        held.fail = () => reject(new Error('The engine stopped.'));
      }),
  );
  return held;
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
});

afterEach(async () => {
  for (const settle of holding.splice(0)) settle();
  await frames();
});

// `bridge.apply` carries no base revision: a drop made before the last commit lands waits for it.
describe('quick drags in a row', () => {
  it("a drag begun before the last drag's commit lands lifts once it does, and commits", async () => {
    const first = holdNextCommit();
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 140);
    await frames();
    fireEvent.pointerDown(title, { pointerId: 1, button: 0, clientX: 140, clientY: 10 });
    fireEvent.pointerMove(title, { pointerId: 1, clientX: 180, clientY: 10 });
    await frames();
    // Nothing is drawn against a project about to be replaced.
    expect(title.dataset.dragging).toBeUndefined();
    await act(async () => first.land({}));
    expect(title.dataset.dragging).toBe('true');
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 180, clientY: 10 });
    await frames();
    expect(bridge.apply).toHaveBeenCalledTimes(2);
  });

  it('two quick moves of a layer both land where they were dropped', async () => {
    const first = holdNextCommit();
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    const item = useProject.getState().snapshot!.project.timeline.tracks[0].items[0];
    await drag(title, 100, 140); // 1 s → 2 s
    await frames();
    await drag(title, 140, 180); // dropped before the first lands: 2 s → 3 s
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    await act(async () => first.land({ items: [{ ...item, startUs: 2_000_000 }] }));
    await frames();
    expect(ops(0)).toMatchObject([{ type: 'moveItem', id: 'title', startUs: 2_000_000 }]);
    expect(ops(1)).toMatchObject([{ type: 'moveItem', id: 'title', startUs: 3_000_000 }]);
  });

  it('two quick reorders both land where they were dropped', async () => {
    // Four 10 s clips (400 px each): d goes to the front, then c, now fourth on screen, one slot left.
    const snapshot = fixtureSnapshot();
    const [, b] = snapshot.project.timeline.main;
    snapshot.project.timeline.main = ['a', 'b', 'c', 'd'].map((id) => ({ ...b, id }));
    useProject.setState({ snapshot });
    const first = holdNextCommit();
    renderWithProviders(<Timeline />);
    drawnAt(byId('d'), 1200);
    await drag(byId('d'), 1300, 100);
    await frames();
    drawnAt(byId('c'), 1200);
    await drag(byId('c'), 1300, 900);
    await act(async () => first.land({ main: ['d', 'a', 'b', 'c'] }));
    await frames();
    expect(ops(0)).toEqual([{ type: 'moveClip', id: 'd', index: 0 }]);
    // d, a, c, b: after d and a, not one slot further left.
    expect(ops(1)).toEqual([{ type: 'moveClip', id: 'c', index: 2 }]);
  });

  it('a drop made while the last commit fails still commits, against the project as it is', async () => {
    const first = holdNextCommit();
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 140);
    await frames();
    await drag(title, 140, 180);
    await act(async () => first.fail());
    await frames();
    // The first move never happened: the second moves the title 1 s from where it still is.
    expect(ops(1)).toMatchObject([{ type: 'moveItem', id: 'title', startUs: 2_000_000 }]);
  });

  it('a drop still waiting when the timeline goes while the project stays open (the recorder) goes in, in its turn', async () => {
    const first = holdNextCommit();
    const { unmount } = renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    await drag(title, 100, 140);
    await frames();
    await drag(title, 140, 180);
    // The recorder takes the editor's place: the project stays open, so the drop already made isn't lost unsaid.
    unmount();
    await act(async () => first.land({}));
    await frames();
    expect(bridge.apply).toHaveBeenCalledTimes(2);
    expect(ops(1)).toEqual([expect.objectContaining({ type: 'moveItem', id: 'title' })]);
  });

  it('a drop still waiting when the project closes goes in before the close, never after it', async () => {
    // As the editor does: the timeline shows while a project is open.
    function Editor() {
      return useProject((s) => s.snapshot !== null) ? <Timeline /> : null;
    }
    const first = holdNextCommit();
    renderWithProviders(<Editor />);
    const title = block(/Set up billing/);
    await drag(title, 100, 140);
    await frames();
    await drag(title, 140, 180);
    const closing = useProject.getState().close();
    await act(async () => first.land({}));
    await act(async () => closing);
    await frames();
    // The close waits for it, as quitting does (closing used to drop it).
    expect(bridge.apply).toHaveBeenCalledTimes(2);
    expect(ops(1)).toEqual([expect.objectContaining({ type: 'moveItem', id: 'title' })]);
    expect(bridge.closeProject).toHaveBeenCalledTimes(1);
    expect(vi.mocked(bridge.apply).mock.invocationCallOrder[1]).toBeLessThan(
      vi.mocked(bridge.closeProject).mock.invocationCallOrder[0],
    );
  });

  it("two quick trims of a clip's end both land: the second doesn't undo the first", async () => {
    const first = holdNextCommit();
    renderWithProviders(<Timeline />);
    const b = block(/^broll\.mp4/);
    const end = () => b.querySelector<HTMLElement>('[data-trim="end"]')!;
    // b is 20 to 30 s (800 to 1200 px): its end goes to 29 s, then 28.5 s.
    await drag(b, 1199, 1159, end());
    await frames();
    await drag(b, 1159, 1139, end());
    const clip = useProject.getState().snapshot!.project.timeline.main[1];
    await act(async () => first.land({ clips: [{ ...clip, sourceOutUs: 9_000_000 }] }));
    await frames();
    expect(ops(0)).toEqual([{ type: 'trimClip', id: 'b', edge: 'end', sourceUs: 9_000_000 }]);
    expect(ops(1)).toEqual([{ type: 'trimClip', id: 'b', edge: 'end', sourceUs: 8_500_000 }]);
  });
});
