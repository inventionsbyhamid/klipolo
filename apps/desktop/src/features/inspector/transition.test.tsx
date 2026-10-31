import { act, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Op } from '../../bindings/Op';
import type { Transition } from '../../bindings/Transition';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The transition card: a selected transition's type, length, settings and Remove in the docked inspector card, each
 * one labelled change (one undo step) built at its turn through the commit queue.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Inspector } = await import('./Inspector');
const { inspectedTransition } = await import('./TransitionInspector');

const DISSOLVE = 'klipolo.builtin/transition.dissolve';
const WIPE = 'klipolo.builtin/transition.wipe';
const DIP = 'klipolo.builtin/transition.dip';

/** The fixture with "b" starting 2 s into its file (2 s of footage before it) and `transition` from "a" into "b". */
function withTransition(transition: Transition) {
  const s = fixtureSnapshot();
  const [a, b] = s.project.timeline.main;
  s.project.timeline.main = [{ ...a, transitionOut: transition }, { ...b, sourceInUs: 2_000_000 }];
  return s;
}

/** The transition each call's one op sets. */
const set = (call: Parameters<typeof apply>) => {
  const op = call[1][0];
  if (op.type !== 'setTransition') throw new Error(`not a setTransition: ${op.type}`);
  return op.transition;
};

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({
    snapshot: withTransition({ effect: DISSOLVE, durationUs: 1_000_000, params: {} }),
    pending: 0,
    apply,
  });
  useSelection.setState({ selectedId: 'transition:a' });
  usePlayback.setState({ playheadUs: 19_000_000, playing: false });
});

describe('the transition card', () => {
  it('shows for a selected transition: its type, its length up to what the footage allows, and Remove', () => {
    renderWithProviders(<Inspector format="landscape" />);
    const card = screen.getByRole('complementary', { name: 'Transition settings' });
    expect(within(card).getByRole('heading', { name: 'Transition' })).toBeInTheDocument();
    expect(within(card).getByRole('button', { name: 'Type: Dissolve' })).toBeInTheDocument();
    const length = within(card).getByRole('slider', { name: 'Length' });
    expect(length).toHaveAttribute('aria-valuenow', '1');
    expect(length).toHaveAttribute('aria-valuetext', '1.0 s');
    // 2 s of spare footage before "b": up to 4 s (and a microsecond, the odd one after the cut).
    expect(length).toHaveAttribute('aria-valuemax', '4.000001');
    expect(within(card).getByRole('button', { name: 'Remove transition' })).toBeInTheDocument();
    // A dissolve has no settings.
    expect(within(card).queryByRole('radiogroup')).toBeNull();
  });

  it('shows nothing for a transition that is gone, or a selection that names none', () => {
    const { project } = fixtureSnapshot();
    expect(inspectedTransition(project, 'transition:a')).toBeNull();
    expect(inspectedTransition(project, 'a')).toBeNull();
    // The last clip has no next clip to blend into.
    const last = { ...project, timeline: { ...project.timeline } };
    last.timeline.main = [
      project.timeline.main[0],
      { ...project.timeline.main[1], transitionOut: { effect: DISSOLVE, durationUs: 500_000, params: {} } },
    ];
    expect(inspectedTransition(last, 'transition:b')).toBeNull();
  });

  it('a new type keeps the length and starts from its own settings; one undo step', async () => {
    useProject.setState({
      snapshot: withTransition({ effect: DIP, durationUs: 1_000_000, params: { color: '#FFFFFF' } }),
    });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Type: Dip to colour' }));
    await userEvent.click(await screen.findByRole('menuitemradio', { name: 'Wipe' }));
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toBe('transition.change.undo');
    expect(set(apply.mock.calls[0])).toEqual({ effect: WIPE, durationUs: 1_000_000, params: {} });
  });

  it('the length stays on the frame grid within the footage: End commits 120 frames, 4 s', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    const length = screen.getByRole('slider', { name: 'Length' });
    act(() => length.focus());
    fireEvent.keyDown(length, { key: 'End' });
    await act(() => new Promise((r) => setTimeout(r, 450)));
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toBe('transition.length.undo');
    expect(set(apply.mock.calls[0])).toEqual({ effect: DISSOLVE, durationUs: 4_000_000, params: {} });
  });

  it("a wipe's direction and softness are its settings, each one change that keeps the others", async () => {
    useProject.setState({
      snapshot: withTransition({ effect: WIPE, durationUs: 1_000_000, params: { softness: 0.3 } }),
    });
    renderWithProviders(<Inspector format="landscape" />);
    const direction = screen.getByRole('radiogroup', { name: 'Direction' });
    // The default shows while none is stored.
    expect(within(direction).getByRole('radio', { name: 'Left' })).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(within(direction).getByRole('radio', { name: 'Up' }));
    expect(apply.mock.calls[0][0]).toBe('transition.change.undo');
    expect(set(apply.mock.calls[0])).toEqual({
      effect: WIPE,
      durationUs: 1_000_000,
      params: { softness: 0.3, direction: 'up' },
    });
    const softness = screen.getByRole('slider', { name: 'Softness' });
    expect(softness).toHaveAttribute('aria-valuetext', '30%');
  });

  it("a dip's colour is picked from the swatches", async () => {
    useProject.setState({ snapshot: withTransition({ effect: DIP, durationUs: 1_000_000, params: {} }) });
    renderWithProviders(<Inspector format="landscape" />);
    const color = screen.getByRole('radiogroup', { name: 'Color' });
    // Black, the dip's default, is checked.
    expect(within(color).getByRole('radio', { name: 'Black' })).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(within(color).getByRole('radio', { name: 'White' }));
    expect(apply.mock.calls[0][0]).toBe('transition.change.undo');
    expect(set(apply.mock.calls[0])).toEqual({ effect: DIP, durationUs: 1_000_000, params: { color: '#FFFFFF' } });
  });

  it('Remove takes the transition out in one step; Close and Escape close the card', async () => {
    const { unmount } = renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Remove transition' }));
    await act(() => new Promise((r) => setTimeout(r, 40)));
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toBe('transition.remove');
    expect(apply.mock.calls[0][1]).toEqual([{ type: 'setTransition', clipId: 'a', transition: null }]);
    unmount();
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(useSelection.getState().selectedId).toBeNull();
    expect(screen.queryByRole('complementary')).toBeNull();
    act(() => useSelection.setState({ selectedId: 'transition:a' }));
    const card = screen.getByRole('complementary', { name: 'Transition settings' });
    fireEvent.keyDown(within(card).getByRole('button', { name: 'Remove transition' }), { key: 'Escape' });
    expect(useSelection.getState().selectedId).toBeNull();
  });

  it('a type this build lacks stays listed by name, with its length', () => {
    useProject.setState({
      snapshot: withTransition({ effect: 'com.acme/fancy', durationUs: 1_000_000, params: {} }),
    });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('button', { name: 'Type: Missing transition' })).toBeInTheDocument();
    expect(screen.getByRole('slider', { name: 'Length' })).toHaveAttribute('aria-valuenow', '1');
  });
});
