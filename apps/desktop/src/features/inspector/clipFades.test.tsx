import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Clip } from '../../bindings/Clip';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The clip card's fades: in from black and out to black, None, 0.5 s or 1 s at each end, one change each. A length
 * that would make the two fades longer than the clip is off, and so is the end where a transition blends the cut (it
 * replaces the fade there); a note under them says why.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
const fill = { mode: 'fill', focusX: 0.5, focusY: 0.5 } as const;
const whole = { x: 0, y: 0, w: 1, h: 1 };
vi.mock('../../bridge/framing', () => ({
  clipFraming: vi.fn(() =>
    Promise.resolve({
      framing: { landscape: fill, portrait: fill, square: fill },
      follows: false,
      fillCrop: { landscape: whole, portrait: whole, square: whole },
    }),
  ),
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Inspector } = await import('./Inspector');

const queued = expect.objectContaining({ projectId: 'project', revision: 3 });
const dissolve = { effect: 'klipolo.builtin/transition.dissolve', durationUs: 500_000, params: {} };

/** The fixture with clip a (the recording, 20 s) and clip b (broll.mp4, 10 s) changed by `a` and `b`. */
function withClips(a: Partial<Clip>, b: Partial<Clip>) {
  const snapshot = fixtureSnapshot();
  const main = snapshot.project.timeline.main;
  main[0] = { ...main[0], ...a };
  main[1] = { ...main[1], ...b };
  return snapshot;
}

const fadeIn = () => screen.getByRole('radiogroup', { name: 'Fade in' });
const fadeOut = () => screen.getByRole('radiogroup', { name: 'Fade out' });

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useSelection.setState({ selectedId: 'b' });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
});

describe('the clip card’s fades', () => {
  it('fade in from black and out to black: None, 0.5 s or 1 s at each end, one change each', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    for (const group of [fadeIn(), fadeOut()]) {
      expect(within(group).getAllByRole('radio').map((r) => r.textContent)).toEqual(['None', '0.5 s', '1 s']);
      expect(within(group).getByRole('radio', { name: 'None' })).toHaveAttribute('aria-checked', 'true');
    }
    await userEvent.click(within(fadeIn()).getByRole('radio', { name: '0.5 s' }));
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith(
        'clip.fades.undo',
        [{ type: 'updateClip', id: 'b', patch: { fadeInUs: 500_000 } }],
        undefined,
        queued,
      ),
    );
    await userEvent.click(within(fadeOut()).getByRole('radio', { name: '1 s' }));
    await waitFor(() =>
      expect(apply).toHaveBeenLastCalledWith(
        'clip.fades.undo',
        [{ type: 'updateClip', id: 'b', patch: { fadeOutUs: 1_000_000 } }],
        undefined,
        queued,
      ),
    );
    expect(screen.queryByText('Longer than the clip.')).toBeNull();
    expect(screen.queryByText('A transition replaces the fade here.')).toBeNull();
  });

  it('a fade set elsewhere shows as it is', () => {
    useProject.setState({ snapshot: withClips({}, { fadeOutUs: 300_000 }) });
    renderWithProviders(<Inspector format="landscape" />);
    expect(within(fadeOut()).getAllByRole('radio').map((r) => r.textContent)).toEqual([
      'None',
      '0.3 s',
      '0.5 s',
      '1 s',
    ]);
    expect(within(fadeOut()).getByRole('radio', { name: '0.3 s' })).toHaveAttribute('aria-checked', 'true');
  });

  it('a length the clip can’t hold beside its other fade is off, and the note says why', async () => {
    // b plays 1.2 s and fades out over 1 s: a fade in of 0.5 s or 1 s would make them longer than the clip.
    useProject.setState({ snapshot: withClips({}, { sourceOutUs: 1_200_000, fadeOutUs: 1_000_000 }) });
    renderWithProviders(<Inspector format="landscape" />);
    const note = screen.getByText('Longer than the clip.');
    expect(within(fadeIn()).getByRole('radio', { name: 'None' })).toBeEnabled();
    for (const name of ['0.5 s', '1 s']) {
      const radio = within(fadeIn()).getByRole('radio', { name });
      expect(radio).toBeDisabled();
      expect(radio).toHaveAttribute('aria-describedby', note.id);
    }
    // The fade out it has fits: every length out is still there to pick (None beside no fade in).
    expect(within(fadeOut()).getByRole('radio', { name: '1 s' })).toBeEnabled();
    await userEvent.click(within(fadeIn()).getByRole('radio', { name: '1 s' }));
    expect(apply).not.toHaveBeenCalled();
  });

  it('a transition at a cut replaces the fade there: that end is off and says so', () => {
    // a dissolves into b: b can't fade in, nor a fade out; b's fade out (into nothing) is free.
    useProject.setState({ snapshot: withClips({ transitionOut: dissolve }, {}) });
    const { unmount } = renderWithProviders(<Inspector format="landscape" />);
    const note = screen.getByText('A transition replaces the fade here.');
    for (const radio of within(fadeIn()).getAllByRole('radio')) {
      expect(radio).toBeDisabled();
      expect(radio).toHaveAttribute('aria-describedby', note.id);
    }
    for (const radio of within(fadeOut()).getAllByRole('radio')) expect(radio).toBeEnabled();
    unmount();
    useSelection.setState({ selectedId: 'a' });
    renderWithProviders(<Inspector format="landscape" />);
    for (const radio of within(fadeOut()).getAllByRole('radio')) expect(radio).toBeDisabled();
    for (const radio of within(fadeIn()).getAllByRole('radio')) expect(radio).toBeEnabled();
    expect(screen.getByText('A transition replaces the fade here.')).toBeInTheDocument();
  });

  it('the last clip’s transition has no cut to blend: its fade out is free', () => {
    useProject.setState({ snapshot: withClips({}, { transitionOut: dissolve }) });
    renderWithProviders(<Inspector format="landscape" />);
    for (const radio of within(fadeOut()).getAllByRole('radio')) expect(radio).toBeEnabled();
    expect(screen.queryByText('A transition replaces the fade here.')).toBeNull();
  });
});
