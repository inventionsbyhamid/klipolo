import { act, fireEvent, screen, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/*
 * The card shows in the left column, over the library, while a clip, a layer or a transition is selected (the frame
 * keeps its size: stageSize.test.tsx). A press on the timeline selects at once, so the column waits for the release:
 * the card never comes or goes under the pointer of a trim or a move, folded library or not. A
 * keyboard walk along the lanes passes bare cuts, which select nothing, so the column waits until the walk settles.
 */

vi.mock('../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../bridge/library', () => ({
  library: {
    pack: vi.fn(() => Promise.resolve([])),
    marks: vi.fn(() => Promise.resolve({})),
    picture: vi.fn(() => Promise.reject(new Error('no pictures in tests'))),
    speechRanges: vi.fn(() => Promise.resolve([])),
  },
}));
vi.mock('../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
    play: vi.fn().mockResolvedValue(undefined),
    pause: vi.fn().mockResolvedValue(0),
  },
}));
vi.mock('../bridge/framing', () => ({
  clipFraming: vi.fn(() => new Promise(() => undefined)),
}));
const { bridge } = await import('../bridge/commands');
const { releaseDock, useDockHold } = await import('../state/dockHold');
const { useLibraryPanel } = await import('../state/libraryPanel');
const { useProject } = await import('../state/project');
const { usePlayback } = await import('../state/playback');
const { useSelection } = await import('../state/selection');
const { useTimelineFocus } = await import('../state/timelineFocus');
const { useUi } = await import('../state/ui');
const { Preview } = await import('../preview/Preview');
const { Library } = await import('./Library');
const { Timeline } = await import('../features/timeline/Timeline');

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));
/** A block on the timeline (the folded library's rail names its clips too). */
const block = (name: RegExp) => within(document.querySelector<HTMLElement>('.timeline')!).getByRole('button', { name });
const edge = (el: HTMLElement, which: 'start' | 'end') => el.querySelector<HTMLElement>(`[data-trim="${which}"]`)!;
/** The left column shows the card over the library. */
const docked = () => document.querySelector('.library[data-card]') !== null;

const press = (el: HTMLElement, x: number) =>
  fireEvent.pointerDown(el, { pointerId: 1, button: 0, clientX: x, clientY: 10 });
const move = (el: HTMLElement, x: number) => fireEvent.pointerMove(el, { pointerId: 1, clientX: x, clientY: 10 });
const release = (el: HTMLElement, x: number) =>
  fireEvent.pointerUp(el, { pointerId: 1, clientX: x, clientY: 10 });

function Editor() {
  return (
    <>
      <Library />
      <Preview />
      <Timeline />
    </>
  );
}

beforeEach(() => {
  releaseDock();
  useDockHold.setState({ walk: false });
  useLibraryPanel.setState({ collapsed: false, aside: null });
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  useTimelineFocus.setState({ activeId: null, pending: false });
  usePlayback.setState({ playheadUs: 25_000_000, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: true, previewFormat: null });
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

describe('the column holds still through a timeline press', () => {
  it("a trim on a clip that wasn't selected keeps the library until the release, then shows the card", async () => {
    renderWithProviders(<Editor />);
    expect(docked()).toBe(false);
    const a = block(/^Recording 1/);
    press(edge(a, 'end'), 799);
    // The press selects the clip at once, but the column keeps the library under the pointer.
    expect(useSelection.getState().selectedId).toBe('a');
    expect(docked()).toBe(false);
    move(a, 759);
    await frames();
    move(a, 719);
    await frames();
    expect(docked()).toBe(false);
    expect(screen.queryByRole('complementary', { name: 'Recording 1 settings' })).toBeNull();
    release(a, 719);
    await frames();
    expect(docked()).toBe(true);
    expect(screen.getByRole('complementary', { name: 'Recording 1 settings' })).toBeInTheDocument();
  });

  it('a move press keeps the column as it was until the release; a plain click shows the card once it lets go', async () => {
    renderWithProviders(<Editor />);
    const b = block(/^broll\.mp4/);
    press(b, 1000);
    expect(useSelection.getState().selectedId).toBe('b');
    expect(docked()).toBe(false);
    move(b, 1100);
    await frames();
    expect(docked()).toBe(false);
    fireEvent.keyDown(window, { key: 'Escape' });
    release(b, 1100);
    await frames();
    expect(docked()).toBe(true);
    // A still click: the card shows at the release.
    act(() => useSelection.getState().select(null));
    expect(docked()).toBe(false);
    const a = block(/^Recording 1/);
    press(a, 100);
    expect(docked()).toBe(false);
    release(a, 100);
    expect(docked()).toBe(true);
  });

  it('a trim on another clip while a card shows keeps the card throughout', async () => {
    useSelection.setState({ selectedId: 'b' });
    renderWithProviders(<Editor />);
    expect(docked()).toBe(true);
    const a = block(/^Recording 1/);
    press(edge(a, 'end'), 799);
    move(a, 759);
    await frames();
    expect(docked()).toBe(true);
    release(a, 759);
    await frames();
    expect(docked()).toBe(true);
  });

  it('a release lost to another app lets the column follow the selection once the window loses focus', async () => {
    renderWithProviders(<Editor />);
    const a = block(/^Recording 1/);
    press(a, 100);
    expect(useSelection.getState().selectedId).toBe('a');
    expect(docked()).toBe(false);
    // Command Tab with the button held: the release goes to the other app, and this window only sees its blur.
    fireEvent.blur(window);
    await frames();
    expect(useDockHold.getState().press).toBe(false);
    expect(docked()).toBe(true);
  });
});

describe('a folded library through a press', () => {
  it('shows no card while the press lasts, then the card at the release, the library still folded', async () => {
    useLibraryPanel.setState({ collapsed: true });
    renderWithProviders(<Editor />);
    const rail = () => document.querySelector<HTMLElement>('.library-rail')!;
    expect(rail().hidden).toBe(false);
    const a = block(/^Recording 1/);
    press(a, 100);
    expect(useSelection.getState().selectedId).toBe('a');
    expect(docked()).toBe(false);
    expect(rail().hidden).toBe(false);
    release(a, 100);
    await frames();
    expect(docked()).toBe(true);
    expect(rail().hidden).toBe(true);
    // The library stays folded: the card lies over the preview's edge.
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    act(() => useSelection.getState().select(null));
    expect(docked()).toBe(false);
    expect(rail().hidden).toBe(false);
  });
});

describe('a press on a block brings its card back', () => {
  it('even on a member of the selected group, whose selection the press doesn’t change', async () => {
    renderWithProviders(<Editor />);
    act(() => useSelection.getState().selectMany(['a', 'b'], 'b'));
    await frames();
    expect(docked()).toBe(true);
    act(() => useLibraryPanel.getState().stepAside());
    expect(docked()).toBe(false);
    const b = block(/^broll\.mp4/);
    press(b, 1000);
    release(b, 1000);
    await frames();
    expect(docked()).toBe(true);
  });
});

describe('the column holds still while the keyboard walks the Video lane', () => {
  const cut = () => document.querySelector<HTMLElement>('[data-block-id="cut:a"]')!;
  const step = async (key: 'ArrowLeft' | 'ArrowRight') => {
    fireEvent.keyDown(document.activeElement!, { key });
    await frames();
  };
  const settle = () => act(() => new Promise((r) => setTimeout(r, 450)));

  it('arrowing across a bare cut never takes the card away; the column follows once the walk stops', async () => {
    useSelection.setState({ selectedId: 'a' });
    renderWithProviders(<Editor />);
    expect(docked()).toBe(true);
    act(() => block(/^Recording 1/).focus());
    // Clip, bare cut (nothing selected), clip: the card stays at every step.
    await step('ArrowRight');
    expect(cut()).toHaveFocus();
    expect(useSelection.getState().selectedId).toBeNull();
    expect(docked()).toBe(true);
    await step('ArrowRight');
    expect(useSelection.getState().selectedId).toBe('b');
    expect(docked()).toBe(true);
    expect(screen.getByRole('complementary', { name: 'broll.mp4 settings' })).toBeInTheDocument();
    // The walk stops on the cut: once it settles, nothing is selected and the card goes.
    await step('ArrowLeft');
    expect(cut()).toHaveFocus();
    expect(docked()).toBe(true);
    await settle();
    expect(docked()).toBe(false);
    // From there onto a clip: the card shows once the walk settles there, not at the step.
    await step('ArrowRight');
    expect(useSelection.getState().selectedId).toBe('b');
    expect(docked()).toBe(false);
    await settle();
    expect(docked()).toBe(true);
  });
});
