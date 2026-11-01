import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Clip } from '../../bindings/Clip';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * Looks: colour presets for the video. The left panel's Looks view shows them as cards: picked, a look goes on the
 * selected clip, or on every clip while no clip is selected, as one labelled step. The clip card names the clip's look,
 * and its Look button opens the Looks view.
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
const { useCaptionsView } = await import('../../state/captionsView');
const { Library } = await import('../../layout/Library');
const { Inspector } = await import('../inspector/Inspector');
const { LooksPanel } = await import('./LooksPanel');

const queued = expect.objectContaining({ projectId: 'project', revision: 3 });
const WARM = 'klipolo.builtin/look.warm';
const COOL = 'klipolo.builtin/look.cool';
const look = (preset: string | null) => ({ preset, strength: 1, lutId: null, lutStrength: 1 });

/** The fixture with clip a (the recording) and clip b (broll.mp4) changed by `a` and `b`. */
function withClips(a: Partial<Clip>, b: Partial<Clip>) {
  const snapshot = fixtureSnapshot();
  const main = snapshot.project.timeline.main;
  main[0] = { ...main[0], ...a };
  main[1] = { ...main[1], ...b };
  return snapshot;
}

const cards = () => within(screen.getByRole('radiogroup', { name: 'Look' })).getAllByRole('radio');
const card = (name: string) => within(screen.getByRole('radiogroup', { name: 'Look' })).getByRole('radio', { name });

beforeEach(() => {
  vi.clearAllMocks();
  mockIPC(() => null);
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useCaptionsView.setState({ view: 'library', open: false });
});

describe('the Looks view', () => {
  it('shows None and the seven looks; with no clip selected, a look goes on every clip as one step', async () => {
    renderWithProviders(<LooksPanel />);
    expect(cards().map((c) => c.getAttribute('aria-label'))).toEqual([
      'None',
      'Warm',
      'Cool',
      'Mono',
      'Vintage',
      'Vivid',
      'Cinema',
      'Rom-com',
    ]);
    expect(card('None')).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByText('Applies to every clip.')).toBeInTheDocument();
    await userEvent.click(card('Warm'));
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith(
        'looks.apply.undo',
        [
          { type: 'updateClip', id: 'a', patch: { look: look(WARM) } },
          { type: 'updateClip', id: 'b', patch: { look: look(WARM) } },
        ],
        { look: WARM },
        queued,
      ),
    );
  });

  it('with a clip selected, it goes on that clip alone; None takes it off', async () => {
    useProject.setState({ snapshot: withClips({}, { look: look(WARM) }) });
    useSelection.setState({ selectedId: 'b' });
    renderWithProviders(<LooksPanel />);
    expect(screen.getByText('Applies to the selected clip.')).toBeInTheDocument();
    expect(card('Warm')).toHaveAttribute('aria-checked', 'true');
    expect(card('None')).toHaveAttribute('aria-checked', 'false');
    await userEvent.click(card('None'));
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith(
        'looks.clear.undo',
        [{ type: 'updateClip', id: 'b', patch: { look: look(null) } }],
        undefined,
        queued,
      ),
    );
  });

  it('picking the look every clip has already makes no step', async () => {
    renderWithProviders(<LooksPanel />);
    await userEvent.click(card('None'));
    expect(apply).not.toHaveBeenCalled();
  });

  it('clips with different looks show none chosen, and Tab reaches the first card', () => {
    useProject.setState({ snapshot: withClips({ look: look(WARM) }, { look: look(COOL) }) });
    renderWithProviders(<LooksPanel />);
    for (const c of cards()) expect(c).toHaveAttribute('aria-checked', 'false');
    expect(cards().map((c) => c.tabIndex)).toEqual([0, -1, -1, -1, -1, -1, -1, -1]);
  });

  it('without a clip there is nothing to try a look on', () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.main = [];
    useProject.setState({ snapshot });
    renderWithProviders(<LooksPanel />);
    for (const c of cards()) expect(c).toBeDisabled();
    expect(screen.getByText('Add a video to try a look.')).toBeInTheDocument();
  });

  it('a look this version lacks is named, with a note that the video shows without it', () => {
    useProject.setState({ snapshot: withClips({}, { look: look('lib.grades/look.teal') }) });
    useSelection.setState({ selectedId: 'b' });
    renderWithProviders(<LooksPanel />);
    const note = screen.getByText('Missing look: this look isn’t installed, so the video shows without it.');
    expect(note).toHaveAttribute('title', 'lib.grades/look.teal');
    for (const c of cards()) expect(c).toHaveAttribute('aria-checked', 'false');
  });

  it('the arrow keys move between the cards, a row of two up or down', async () => {
    renderWithProviders(<LooksPanel />);
    act(() => card('None').focus());
    await userEvent.keyboard('{ArrowRight}');
    expect(card('Warm')).toHaveFocus();
    await userEvent.keyboard('{ArrowDown}');
    expect(card('Mono')).toHaveFocus();
    await userEvent.keyboard('{ArrowLeft}');
    expect(card('Cool')).toHaveFocus();
    await userEvent.keyboard('{ArrowUp}');
    expect(card('None')).toHaveFocus();
    // The last card stays the last.
    act(() => card('Rom-com').focus());
    await userEvent.keyboard('{ArrowRight}{ArrowDown}');
    expect(card('Rom-com')).toHaveFocus();
    expect(apply).not.toHaveBeenCalled();
  });

  it('its swatches use tokens for the card and the look’s own colours for the swatch', () => {
    const css = readFileSync(resolve(__dirname, 'looks.css'), 'utf8');
    expect(css).not.toMatch(/--color-cut/);
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(/i);
  });
});

describe('the left panel’s Looks tab', () => {
  it('opens beside Library and Captions, and gives the Library back', async () => {
    renderWithProviders(<Library />);
    expect(screen.getAllByRole('tab', { name: /^(Library|Captions|Looks)$/ }).map((t) => t.textContent)).toEqual([
      'Library',
      'Captions',
      'Looks',
    ]);
    await userEvent.click(screen.getByRole('tab', { name: 'Looks' }));
    expect(screen.getByRole('tab', { name: 'Looks' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('complementary', { name: 'Looks' })).toHaveAttribute('data-view', 'looks');
    expect(screen.getByRole('radiogroup', { name: 'Look' })).toBeInTheDocument();
    expect(screen.queryByRole('tab', { name: 'Clips' })).toBeNull();
    await userEvent.click(screen.getByRole('tab', { name: 'Library' }));
    expect(screen.queryByRole('radiogroup', { name: 'Look' })).toBeNull();
    expect(screen.getByRole('tab', { name: 'Clips' })).toBeInTheDocument();
  });
});

describe('the clip card’s Look', () => {
  it('names the clip’s look, and opens the Looks view', async () => {
    useProject.setState({ snapshot: withClips({}, { look: look(WARM) }) });
    useSelection.setState({ selectedId: 'b' });
    renderWithProviders(<Inspector format="landscape" />);
    const button = screen.getByRole('button', { name: 'Look: Warm' });
    await userEvent.click(button);
    expect(useCaptionsView.getState().view).toBe('looks');
    expect(apply).not.toHaveBeenCalled();
  });

  it('a clip without a look reads None', () => {
    useSelection.setState({ selectedId: 'b' });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('button', { name: 'Look: None' })).toBeInTheDocument();
  });
});
