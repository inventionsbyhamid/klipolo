import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SnapshotPatch } from '../../bindings/SnapshotPatch';
import type { Transaction } from '../../bindings/Transaction';
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
const { useToasts } = await import('../../state/toasts');
const { Timeline } = await import('./Timeline');
const { Inspector } = await import('../inspector/Inspector');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');
const { labelText } = await import('../../lib/labels');

const appCss = readFileSync(resolve(__dirname, '../../styles/app.css'), 'utf8');
/** The declarations of the rule whose selector list is exactly `selector` (as written, one selector a line). */
function rule(selector: string): string {
  for (const block of appCss.split('}')) {
    const at = block.lastIndexOf('{');
    const head = block
      .slice(0, at)
      .replace(/\/\*[\s\S]*?\*\//g, '')
      .trim();
    if (at >= 0 && head === selector) return block.slice(at + 1);
  }
  throw new Error(`app.css has no rule for ${selector}`);
}

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));
const DISSOLVE = 'klipolo.builtin/transition.dissolve';
const WIPE = 'klipolo.builtin/transition.wipe';

function ShortcutsHarness() {
  useEditorShortcuts();
  return null;
}

/** Clip "b" starts 2 s into broll (10 s long), so it plays 8 s and has 2 s of footage before it. */
const withHandles = () => {
  const s = fixtureSnapshot();
  s.project.timeline.main[1] = { ...s.project.timeline.main[1], sourceInUs: 2_000_000 };
  return s;
};

/** `withHandles` with a one-second dissolve from "a" into "b". */
const withDissolve = () => {
  const s = withHandles();
  s.project.timeline.main[0] = {
    ...s.project.timeline.main[0],
    transitionOut: { effect: DISSOLVE, durationUs: 1_000_000, params: {} },
  };
  return s;
};

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  useTimelineFocus.setState({ activeId: null, pending: false });
  usePlayback.setState({ playheadUs: 25_000_000, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: true, previewFormat: null });
  useToasts.setState({ toasts: [] });
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

describe('transitions at cuts', () => {
  it('a cut offers Add transition and adds a half-second dissolve', async () => {
    useProject.setState({ snapshot: withHandles() });
    renderWithProviders(<Timeline />);
    await userEvent.click(screen.getByRole('button', { name: 'Add transition between Recording 1 and broll.mp4' }));
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'transition.add',
      ops: [{ type: 'setTransition', clipId: 'a', transition: { effect: DISSOLVE, durationUs: 500_000, params: {} } }],
    });
    // The new transition is selected, and the live region says what went in.
    expect(useSelection.getState().selectedId).toBe('transition:a');
    expect(screen.getByRole('status')).toHaveTextContent('Added a dissolve between Recording 1 and broll.mp4');
  });

  it('a cut without spare footage explains instead of offering', () => {
    renderWithProviders(<Timeline />);
    expect(screen.queryByRole('button', { name: /Add transition/ })).toBeNull();
    expect(screen.getByLabelText(/Not enough extra footage here for a transition/)).toBeInTheDocument();
  });

  it('a transition is a chip as wide as it lasts that opens its card', async () => {
    useProject.setState({ snapshot: withDissolve() });
    renderWithProviders(
      <>
        <Timeline />
        <Inspector format="landscape" />
      </>,
    );
    const chip = screen.getByRole('button', { name: /Dissolve transition, 1.0 s, between Recording 1 and broll\.mp4/ });
    // 1 s at 40 px/s, centred on the cut at 20 s.
    expect(chip).toHaveStyle({ width: '40px', left: '780px' });
    await userEvent.click(chip);
    expect(useSelection.getState().selectedId).toBe('transition:a');
    expect(chip).toHaveAttribute('aria-pressed', 'true');
    const card = screen.getByRole('complementary', { name: 'Transition settings' });
    expect(card).toBeInTheDocument();
    // 2 s of spare footage before "b": up to 4.0 s.
    expect(screen.getByRole('slider', { name: 'Length' })).toHaveAttribute('aria-valuemax', '4.000001');
    await userEvent.click(screen.getByRole('button', { name: 'Remove transition' }));
    await frames();
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'transition.remove',
      ops: [{ type: 'setTransition', clipId: 'a', transition: null }],
    });
  });

  it('⌘T adds a transition at the cut nearest the playhead', async () => {
    useProject.setState({ snapshot: withHandles() });
    usePlayback.setState({ playheadUs: 19_000_000 });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    fireEvent.keyDown(window, { key: 't', metaKey: true });
    await frames(); // `useProject.apply` sends from its queue, a microtask later
    expect(vi.mocked(bridge.apply).mock.calls[0][0].label).toBe('transition.add');
  });

  it('Delete on a selected transition removes it; ←/→ visit the cut, and Enter there selects it', async () => {
    useProject.setState({ snapshot: withDissolve() });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    const a = screen.getByRole('button', { name: /^Recording 1/ });
    act(() => a.focus());
    fireEvent.keyDown(a, { key: 'ArrowRight' });
    await frames();
    const chip = screen.getByRole('button', { name: /^Dissolve transition/ });
    expect(chip).toHaveFocus();
    expect(useSelection.getState().selectedId).toBe('transition:a');
    fireEvent.keyDown(chip, { key: 'Delete' });
    await frames();
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'transition.remove',
      ops: [{ type: 'setTransition', clipId: 'a', transition: null }],
    });
    fireEvent.keyDown(chip, { key: 'ArrowRight' });
    await frames();
    expect(screen.getByRole('button', { name: /^broll\.mp4/ })).toHaveFocus();
    expect(useSelection.getState().selectedId).toBe('b');
    // Back to the cut, and Enter there selects its transition again.
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowLeft' });
    await frames();
    act(() => useSelection.getState().select(null));
    fireEvent.keyDown(chip, { key: 'Enter' });
    await frames();
    expect(useSelection.getState().selectedId).toBe('transition:a');
  });

  it('Enter on a bare cut adds a transition; a cut without footage says why', async () => {
    useProject.setState({ snapshot: withHandles() });
    const { unmount } = renderWithProviders(<Timeline />);
    const add = screen.getByRole('button', { name: 'Add transition between Recording 1 and broll.mp4' });
    act(() => add.focus());
    fireEvent.keyDown(add, { key: 'Enter' });
    await frames();
    expect(vi.mocked(bridge.apply).mock.calls[0][0].label).toBe('transition.add');
    unmount();
    useProject.setState({ snapshot: fixtureSnapshot() });
    renderWithProviders(<Timeline />);
    const note = screen.getByLabelText(/Not enough extra footage here for a transition/);
    act(() => note.focus());
    fireEvent.keyDown(note, { key: 'Enter' });
    await frames();
    expect(vi.mocked(bridge.apply).mock.calls).toHaveLength(1);
    expect(useToasts.getState().toasts.at(-1)?.text).toBe(
      'Not enough extra footage here for a transition. Trim the clips at this cut to make room.',
    );
  });

  it('one tip for the cut markers: after a hover’s delay, at once on keyboard focus, gone on a press', async () => {
    renderWithProviders(<Timeline />);
    const note = screen.getByLabelText(/Not enough extra footage here for a transition/);
    const tip = document.querySelector<HTMLElement>('[data-cut-tip]')!;
    fireEvent.pointerOver(note);
    expect(tip.hidden).toBe(true);
    await act(() => new Promise((r) => setTimeout(r, 450)));
    expect(tip.hidden).toBe(false);
    expect(tip).toHaveTextContent('Not enough extra footage here for a transition');
    fireEvent.pointerOut(note, { relatedTarget: document.body });
    expect(tip.hidden).toBe(true);
    act(() => note.focus());
    await act(() => new Promise((r) => setTimeout(r, 0)));
    expect(tip.hidden).toBe(false);
    fireEvent.pointerDown(note, { button: 0 });
    expect(tip.hidden).toBe(true);
  });

  it('a clip’s menu offers Add transition after when the cut after it can take one', async () => {
    useProject.setState({ snapshot: withHandles() });
    renderWithProviders(<Timeline />);
    const a = screen.getByRole('button', { name: /^Recording 1/ });
    act(() => a.focus());
    fireEvent.keyDown(a, { key: 'F10', shiftKey: true });
    await userEvent.click(await screen.findByRole('menuitem', { name: /^Add transition after/ }));
    await frames();
    expect(vi.mocked(bridge.apply).mock.calls[0][0]).toMatchObject({
      label: 'transition.add',
      ops: [{ type: 'setTransition', clipId: 'a' }],
    });
  });

  it('a clip with a transition after it, or none after it, has no Add transition after', async () => {
    useProject.setState({ snapshot: withDissolve() });
    renderWithProviders(<Timeline />);
    for (const name of [/^Recording 1/, /^broll\.mp4/]) {
      const clip = screen.getByRole('button', { name });
      act(() => clip.focus());
      fireEvent.keyDown(clip, { key: 'F10', shiftKey: true });
      expect(await screen.findByRole('menuitem', { name: /^Split at playhead/ })).toBeInTheDocument();
      expect(screen.queryByRole('menuitem', { name: /^Add transition after/ })).toBeNull();
      await userEvent.keyboard('{Escape}');
    }
  });
});

/** The element that last took the pointer (`setPointerCapture`): a browser sends it the moves and the release. */
let captor: Element | null = null;
const capture = vi.spyOn(Element.prototype, 'setPointerCapture');
beforeEach(() => {
  captor = null;
  capture.mockImplementation(function (this: Element) {
    captor = this;
  });
});
afterEach(() => capture.mockReset());

/**
 * A press on marker `el` at `x` px into it (jsdom puts every box at 0, so that is its `clientX`; it has no
 * hit-testing, so the press goes to the marker itself), moved through `moves` with the button down and released at
 * the last, then the click a release makes. The moves and the release go where the pointer is captured, as a browser
 * sends them.
 */
async function pressMarker(el: HTMLElement, x: number, moves: number[] = []) {
  fireEvent.pointerDown(el, { pointerId: 1, button: 0, buttons: 1, clientX: x, clientY: 60 });
  for (const to of moves) {
    fireEvent.pointerMove(captor ?? el, { pointerId: 1, buttons: 1, clientX: to, clientY: 60 });
    await frames();
  }
  const at = moves.at(-1) ?? x;
  const target = captor ?? el;
  fireEvent.pointerUp(target, { pointerId: 1, clientX: at, clientY: 60 });
  fireEvent.click(target, { clientX: at, detail: 1 });
  await frames();
}
const applied = () => vi.mocked(bridge.apply).mock.calls.map((c) => c[0]);
const cutMarker = () => document.querySelector<HTMLElement>('[data-block-id="cut:a"]')!;

describe('a press on a cut marker: a drag trims the clip on that side, a still click acts on the cut', () => {
  it('on the +, a drag from left of the cut trims the clip before it, with nothing moved before the drag', async () => {
    useProject.setState({ snapshot: withHandles() });
    renderWithProviders(<Timeline />);
    const b = screen.getByRole('button', { name: /^broll\.mp4/ });
    act(() => b.focus());
    useSelection.setState({ selectedId: 'b' });
    const add = screen.getByRole('button', { name: 'Add transition between Recording 1 and broll.mp4' });
    // The cut is at the middle of the 20 px spot: 2 px left of it, and 2 px is no move yet.
    fireEvent.pointerDown(add, { pointerId: 1, button: 0, buttons: 1, clientX: 8, clientY: 60 });
    fireEvent.pointerMove(add, { pointerId: 1, buttons: 1, clientX: 6, clientY: 60 });
    expect(b).toHaveFocus();
    expect(useSelection.getState().selectedId).toBe('b');
    expect(captor).toBe(add);
    // 5 px: the press is the trim's, which takes the pointer, focus and selection to a.
    fireEvent.pointerMove(add, { pointerId: 1, buttons: 1, clientX: 3, clientY: 60 });
    await frames();
    const a = screen.getByRole('button', { name: /^Recording 1/ });
    expect(captor).toBe(a);
    expect(a).toHaveFocus();
    fireEvent.pointerMove(a, { pointerId: 1, buttons: 1, clientX: -32, clientY: 60 });
    await frames();
    fireEvent.pointerUp(a, { pointerId: 1, clientX: -32, clientY: 60 });
    // The click that release makes (on the lane, in a browser) neither seeks nor adds.
    const lane = document.querySelector<HTMLElement>('[data-lane="main"] .tl-lane')!;
    fireEvent.click(lane, { clientX: 200, detail: 1 });
    await frames();
    expect(usePlayback.getState().playheadUs).toBe(25_000_000);
    // 40 px at 40 px/s: a ends a second earlier. No transition, no toast.
    expect(applied()).toEqual([
      { label: 'timeline.trim.clip.undo', ops: [{ type: 'trimClip', id: 'a', edge: 'end', sourceUs: 19_000_000 }] },
    ]);
    expect(useToasts.getState().toasts).toEqual([]);
    expect(useSelection.getState().selectedId).toBe('a');
  });

  it('on the +, a drag from just right of the cut trims the start of the clip after it', async () => {
    useProject.setState({ snapshot: withHandles() });
    renderWithProviders(<Timeline />);
    await pressMarker(cutMarker(), 12, [17, 52]);
    // b starts 2 s into its file: a second later.
    expect(applied()).toEqual([
      { label: 'timeline.trim.clip.undo', ops: [{ type: 'trimClip', id: 'b', edge: 'start', sourceUs: 3_000_000 }] },
    ]);
  });

  it('on the +, a still click adds a transition', async () => {
    useProject.setState({ snapshot: withHandles() });
    renderWithProviders(<Timeline />);
    await pressMarker(cutMarker(), 8);
    expect(applied().map((tx) => tx.label)).toEqual(['transition.add']);
    expect(cutMarker()).toHaveFocus();
    expect(useSelection.getState().selectedId).toBe('transition:a');
  });

  it("on a transition's chip, a drag near the cut trims either side; a still click selects it", async () => {
    useProject.setState({ snapshot: withDissolve() });
    const { unmount } = renderWithProviders(<Timeline />);
    // The 40 px chip is centred on the cut: 20 px into it.
    await pressMarker(cutMarker(), 22, [27, 62]);
    await pressMarker(cutMarker(), 18, [13, -22]);
    expect(applied()).toEqual([
      { label: 'timeline.trim.clip.undo', ops: [{ type: 'trimClip', id: 'b', edge: 'start', sourceUs: 3_000_000 }] },
      { label: 'timeline.trim.clip.undo', ops: [{ type: 'trimClip', id: 'a', edge: 'end', sourceUs: 19_000_000 }] },
    ]);
    unmount();
    renderWithProviders(<Timeline />);
    await pressMarker(cutMarker(), 21);
    expect(useSelection.getState().selectedId).toBe('transition:a');
    expect(applied()).toHaveLength(2);
  });

  it('at a cut with no room, a still click says why and a drag trims instead of saying it again', async () => {
    renderWithProviders(<Timeline />);
    await pressMarker(cutMarker(), 9);
    expect(useToasts.getState().toasts.map((toast) => toast.text)).toEqual([
      'Not enough extra footage here for a transition. Trim the clips at this cut to make room.',
    ]);
    useToasts.setState({ toasts: [] });
    await pressMarker(cutMarker(), 11, [16, 51]);
    expect(useToasts.getState().toasts).toEqual([]);
    expect(applied()).toEqual([
      { label: 'timeline.trim.clip.undo', ops: [{ type: 'trimClip', id: 'b', edge: 'start', sourceUs: 1_000_000 }] },
    ]);
  });

  it('a hold whose marker goes before its release ends: a later hover with no button down trims nothing', async () => {
    const cut = withHandles();
    useProject.setState({ snapshot: cut });
    renderWithProviders(<Timeline />);
    fireEvent.pointerDown(cutMarker(), { pointerId: 1, button: 0, buttons: 1, clientX: 8, clientY: 60 });
    // The cut goes mid-hold (an undo), so the release lands elsewhere; then it comes back (a redo).
    const noCut = withHandles();
    noCut.project.timeline.main = noCut.project.timeline.main.slice(0, 1);
    act(() => useProject.setState({ snapshot: noCut }));
    fireEvent.pointerUp(document.body, { pointerId: 1, clientX: 8, clientY: 60 });
    act(() => useProject.setState({ snapshot: cut }));
    // Later the pointer passes over the cut with no button down, then clicks empty lane space.
    fireEvent.pointerMove(cutMarker(), { pointerId: 1, buttons: 0, clientX: 40, clientY: 60 });
    await frames();
    const lane = document.querySelector<HTMLElement>('[data-lane="main"] .tl-lane')!;
    fireEvent.pointerDown(lane, { pointerId: 1, button: 0, buttons: 1, clientX: 200, clientY: 60 });
    fireEvent.pointerUp(lane, { pointerId: 1, clientX: 200, clientY: 60 });
    await frames();
    expect(applied()).toEqual([]);
    expect(useSelection.getState().selectedId).toBeNull();
  });

  it('after a drag from the cut trims, a click on the cut with no press (VoiceOver) still adds', async () => {
    useProject.setState({ snapshot: withHandles() });
    renderWithProviders(<Timeline />);
    await pressMarker(cutMarker(), 12, [17, 52]);
    // VO-Space clicks the cut with no pointer press before it.
    fireEvent.click(cutMarker(), { detail: 0 });
    await frames();
    expect(applied().map((tx) => tx.label)).toEqual(['timeline.trim.clip.undo', 'transition.add']);
  });

  it("a drag from the cut whose release never comes doesn't eat the next click", async () => {
    useProject.setState({ snapshot: withHandles() });
    renderWithProviders(<Timeline />);
    fireEvent.pointerDown(cutMarker(), { pointerId: 1, button: 0, buttons: 1, clientX: 8, clientY: 60 });
    fireEvent.pointerMove(cutMarker(), { pointerId: 1, buttons: 1, clientX: 3, clientY: 60 });
    await frames();
    // The release never reaches the page (Mission Control mid-drag), and Escape puts the edge back.
    fireEvent.keyDown(document.body, { key: 'Escape' });
    // The next click on empty lane space seeks there: 200 px at 40 px/s.
    const lane = document.querySelector<HTMLElement>('[data-lane="main"] .tl-lane')!;
    fireEvent.pointerDown(lane, { pointerId: 1, button: 0, buttons: 1, clientX: 200, clientY: 60 });
    fireEvent.pointerUp(lane, { pointerId: 1, clientX: 200, clientY: 60 });
    fireEvent.click(lane, { clientX: 200, detail: 1 });
    await frames();
    expect(usePlayback.getState().playheadUs).toBe(5_000_000);
    expect(applied()).toEqual([]);
  });
});

/** A stand-in engine that applies `setTransition` to the store after `ms`. */
function engineApplies(ms: number) {
  return (tx: Transaction): Promise<SnapshotPatch> =>
    new Promise((resolve) =>
      setTimeout(() => {
        const s = useProject.getState().snapshot!;
        const clips = s.project.timeline.main.map((clip) => {
          let next = clip;
          for (const op of tx.ops) {
            if (op.type === 'setTransition' && op.clipId === clip.id)
              next = { ...next, transitionOut: op.transition ?? undefined };
          }
          return next;
        });
        resolve({
          baseRevision: s.history.revision,
          history: { ...s.history, revision: s.history.revision + 1 },
          patch: { clips: clips.filter((c, i) => c !== s.project.timeline.main[i]), items: [] },
        });
      }, ms),
    );
}

describe('the cut markers and the clips around them', () => {
  it('the + shows only while its own spot is pointed at or focused, never for a hover elsewhere on the lane', () => {
    useProject.setState({ snapshot: withHandles() });
    renderWithProviders(<Timeline />);
    const add = screen.getByRole('button', { name: 'Add transition between Recording 1 and broll.mp4' });
    expect(add).toHaveClass('tl-cut-spot');
    expect(rule('.tl-cut-spot')).toMatch(/opacity: 0;/);
    expect(rule('.tl-cut-spot:hover,\n.tl-cut-spot:focus-visible')).toMatch(/opacity: 1;/);
  });

  it('adding a transition over a fade says it replaces it, in the tip and the announcement', async () => {
    const s = withHandles();
    s.project.timeline.main[0] = { ...s.project.timeline.main[0], fadeOutUs: 500_000 };
    useProject.setState({ snapshot: s });
    renderWithProviders(<Timeline />);
    const add = screen.getByRole('button', { name: 'Add transition between Recording 1 and broll.mp4' });
    expect(add.dataset.tip).toBe('Add transition (replaces the fade out)');
    await userEvent.click(add);
    await frames();
    expect(screen.getByRole('status')).toHaveTextContent(
      'Added a dissolve between Recording 1 and broll.mp4. It replaces the fade out.',
    );
  });

  it('zoomed out, a transition still shows: a chip between clips of 30 px or more, else a tint at their edges', () => {
    useProject.setState({ snapshot: withDissolve() });
    // 4 px/s: a is 80 px, b 32 px. A bare cut's + would need 40 px; the chip draws.
    useUi.setState({ pxPerSecond: 4 });
    const { unmount } = renderWithProviders(<Timeline />);
    expect(screen.getByRole('button', { name: /^Dissolve transition/ })).toBeInTheDocument();
    unmount();
    // 3 px/s: b is 24 px. No chip: a's end and b's start are tinted for their halves (0.5 s each: 1.5 px, at least 2).
    useUi.setState({ pxPerSecond: 3 });
    renderWithProviders(<Timeline />);
    expect(screen.queryByRole('button', { name: /^Dissolve transition/ })).toBeNull();
    const a = document.querySelector<HTMLElement>('[data-block-id="a"]')!;
    const b = document.querySelector<HTMLElement>('[data-block-id="b"]')!;
    expect(a.dataset.transitionOut).toBe('2');
    expect(b.dataset.transitionIn).toBe('2');
    expect(a.dataset.transitionIn).toBeUndefined();
  });

  it('⌘T and Add transition after never fail silently where there is no room', async () => {
    usePlayback.setState({ playheadUs: 19_000_000 });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    fireEvent.keyDown(window, { key: 't', metaKey: true });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(useToasts.getState().toasts.at(-1)?.text).toMatch(/^Not enough extra footage here for a transition/);
    useToasts.setState({ toasts: [] });
    const a = screen.getByRole('button', { name: /^Recording 1/ });
    act(() => a.focus());
    fireEvent.keyDown(a, { key: 'F10', shiftKey: true });
    const item = await screen.findByRole('menuitem', { name: /^Add transition after/ });
    expect(item).not.toHaveAttribute('aria-disabled');
    await userEvent.click(item);
    await frames();
    expect(useToasts.getState().toasts.at(-1)?.text).toMatch(/^Not enough extra footage here for a transition/);
  });

  it('in a project with one clip, ⌘T and the clip’s menu say a transition goes between two clips', async () => {
    const needsTwo = 'A transition goes between two clips. Split this clip or add another video first.';
    const s = fixtureSnapshot();
    s.project.timeline.main = s.project.timeline.main.slice(0, 1);
    useProject.setState({ snapshot: s });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    fireEvent.keyDown(window, { key: 't', metaKey: true });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(useToasts.getState().toasts.at(-1)?.text).toBe(needsTwo);
    // The clip selected: the same.
    useToasts.setState({ toasts: [] });
    act(() => useSelection.getState().select('a'));
    fireEvent.keyDown(window, { key: 't', metaKey: true });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(useToasts.getState().toasts.at(-1)?.text).toBe(needsTwo);
    // Its menu lists Add transition after, off, with the same note as its tooltip and description.
    const a = screen.getByRole('button', { name: /^Recording 1/ });
    act(() => a.focus());
    fireEvent.keyDown(a, { key: 'F10', shiftKey: true });
    const item = await screen.findByRole('menuitem', { name: /^Add transition after/ });
    expect(item).toHaveAttribute('aria-disabled', 'true');
    expect(item).toHaveAttribute('title', needsTwo);
    expect(item).toHaveAccessibleDescription(needsTwo);
  });

  it('Delete on a clip says what it does to the transitions it touched, in its undo label and aloud', async () => {
    // a, b and a copy of b (c) with 0.3 s of footage before it: the second dissolve into b goes into c once b goes,
    // and c has room for 0.6 s of it.
    const s = withDissolve();
    const [, b] = s.project.timeline.main;
    s.project.timeline.main.push({ ...b, id: 'c', sourceInUs: 300_000 });
    useProject.setState({ snapshot: s });
    useSelection.setState({ selectedId: 'b' });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    fireEvent.keyDown(window, { key: 'Delete' });
    await frames();
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'undo.delete.clip.note',
      params: { note: 'A transition is shortened.' },
      ops: [{ type: 'removeClip', id: 'b' }],
    });
    expect(screen.getByRole('status')).toHaveTextContent('Deleted broll.mp4. A transition is shortened.');
    // The last clip: the transition into it has nowhere to go.
    useProject.setState({ snapshot: withDissolve() });
    act(() => useSelection.getState().select('b'));
    fireEvent.keyDown(window, { key: 'Delete' });
    await frames();
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'undo.delete.clip.note',
      params: { note: 'A transition is removed.' },
      ops: [{ type: 'removeClip', id: 'b' }],
    });
    expect(screen.getByRole('status')).toHaveTextContent('Deleted broll.mp4. A transition is removed.');
    expect(labelText({ id: 'undo.delete.clip.note', params: { note: 'A transition is removed.' } })).toBe(
      'Delete clip. A transition is removed.',
    );
    // A clip with no transition at either cut: only what it did.
    useProject.setState({ snapshot: fixtureSnapshot() });
    act(() => useSelection.getState().select('b'));
    fireEvent.keyDown(window, { key: 'Delete' });
    await frames();
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'undo.delete.clip',
      ops: [{ type: 'removeClip', id: 'b' }],
    });
    expect(screen.getByRole('status')).toHaveTextContent(/^Deleted broll\.mp4$/);
  });

  it('Command X on a clip says what it does to the transitions it touched, as Delete does', async () => {
    // The last clip: the dissolve into it has nowhere to go.
    useProject.setState({ snapshot: withDissolve() });
    useSelection.setState({ selectedId: 'b' });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    fireEvent.keyDown(window, { key: 'x', metaKey: true });
    await frames();
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'undo.cut.note',
      params: { note: 'A transition is removed.' },
      ops: [{ type: 'removeClip', id: 'b' }],
    });
    expect(screen.getByRole('status')).toHaveTextContent('Cut broll.mp4. A transition is removed.');
    expect(labelText({ id: 'undo.cut.note', params: { note: 'A transition is removed.' } })).toBe(
      'Cut. A transition is removed.',
    );
    // A clip with no transition at either cut: only what it did.
    useProject.setState({ snapshot: fixtureSnapshot() });
    act(() => useSelection.getState().select('b'));
    fireEvent.keyDown(window, { key: 'x', metaKey: true });
    await frames();
    expect(bridge.apply).toHaveBeenLastCalledWith({ label: 'undo.cut', ops: [{ type: 'removeClip', id: 'b' }] });
    expect(screen.getByRole('status')).toHaveTextContent(/^Cut broll\.mp4$/);
  });

  it('a type this build lacks reads "Missing transition" once, its id as the tooltip', () => {
    const s = withDissolve();
    s.project.timeline.main[0].transitionOut!.effect = 'com.acme/fancy';
    useProject.setState({ snapshot: s });
    renderWithProviders(<Timeline />);
    expect(
      screen.getByRole('button', { name: 'Missing transition, 1.0 s, between Recording 1 and broll.mp4' }),
    ).toHaveAttribute('title', 'com.acme/fancy');
  });

  it('a length set while a new type is still landing keeps the new type (ops built when they go in)', async () => {
    vi.mocked(bridge.apply).mockImplementation(engineApplies(50));
    useProject.setState({ snapshot: withDissolve() });
    useSelection.setState({ selectedId: 'transition:a' });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Type: Dissolve' }));
    await userEvent.click(await screen.findByRole('menuitemradio', { name: 'Wipe' }));
    // At once, before the wipe has landed: End on Length.
    const length = screen.getByRole('slider', { name: 'Length' });
    act(() => length.focus());
    fireEvent.keyDown(length, { key: 'End' });
    await act(() => new Promise((r) => setTimeout(r, 500)));
    const calls = vi.mocked(bridge.apply).mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[1][0]).toEqual({
      label: 'transition.length.undo',
      ops: [{ type: 'setTransition', clipId: 'a', transition: { effect: WIPE, durationUs: 4_000_000, params: {} } }],
    });
  });

  it("the card's Remove leaves focus on the cut, never the page", async () => {
    vi.mocked(bridge.apply).mockImplementation(engineApplies(0));
    useProject.setState({ snapshot: withDissolve() });
    useSelection.setState({ selectedId: 'transition:a' });
    renderWithProviders(
      <>
        <Timeline />
        <Inspector format="landscape" />
      </>,
    );
    const remove = screen.getByRole('button', { name: 'Remove transition' });
    act(() => remove.focus());
    await userEvent.keyboard('{Enter}');
    await frames();
    expect(useProject.getState().snapshot!.project.timeline.main[0].transitionOut).toBeUndefined();
    // The card has gone with it; the cut it was at has the focus.
    expect(screen.queryByRole('complementary', { name: 'Transition settings' })).toBeNull();
    expect(cutMarker()).toHaveFocus();
  });

  it('a selected transition that goes is no longer selected: removed, or its add undone', async () => {
    vi.mocked(bridge.apply).mockImplementation(engineApplies(0));
    useProject.setState({ snapshot: withDissolve() });
    useSelection.setState({ selectedId: 'transition:a' });
    renderWithProviders(
      <>
        <Timeline />
        <ShortcutsHarness />
      </>,
    );
    // A change that keeps it keeps it selected.
    act(() => useProject.setState({ snapshot: withDissolve() }));
    expect(useSelection.getState().selectedId).toBe('transition:a');
    fireEvent.keyDown(window, { key: 'Delete' });
    await frames();
    expect(useProject.getState().snapshot!.project.timeline.main[0].transitionOut).toBeUndefined();
    expect(useSelection.getState().selectedId).toBeNull();
    // Undo of an add: the project comes back without it.
    act(() => {
      useProject.setState({ snapshot: withDissolve() });
      useSelection.setState({ selectedId: 'transition:a' });
    });
    act(() => useProject.setState({ snapshot: withHandles() }));
    expect(useSelection.getState().selectedId).toBeNull();
  });
});
