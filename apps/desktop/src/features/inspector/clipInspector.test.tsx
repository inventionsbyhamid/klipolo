import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Clip } from '../../bindings/Clip';
import type { ClipFraming } from '../../bindings/ClipFraming';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The clip card: the docked inspector for a selected main-track clip. Fit or Fill in the format shown, as Rust frames
 * it (`clip_framing`), and the Fill focus while Fill crops; each change is one labelled step, built at its turn through
 * the commit queue.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
/**
 * Rust's framing (`clip_framing`): the imported clip (b) fills from the centre in every format; the screen recording
 * (a), which has a cursor here, is fitted in 16:9 and fills following the cursor in 9:16 and 1:1.
 */
const fill = { mode: 'fill', focusX: 0.5, focusY: 0.5 } as const;
const follows = { ...fill, follow: { closer: 1 } };
const FRAMING: Record<string, ClipFraming> = {
  a: {
    framing: { landscape: { mode: 'fit', focusX: 0.5, focusY: 0.5 }, portrait: follows, square: follows },
    follows: true,
  },
  b: { framing: { landscape: fill, portrait: fill, square: fill }, follows: false },
};
vi.mock('../../bridge/framing', () => ({
  clipFraming: vi.fn((clipId: string) => Promise.resolve(FRAMING[clipId])),
}));
const { engine } = await import('../../bridge/engine');
const { clipFraming } = await import('../../bridge/framing');
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Inspector } = await import('./Inspector');

/** Where a change through the commit queue was asked for (`useProject.apply`'s `built`): every edit goes that way. */
const queued = expect.objectContaining({ projectId: 'project', revision: 3 });

/** The fixture with clip b (broll.mp4) changed by `patch`. */
function withB(patch: Partial<Clip>) {
  const snapshot = fixtureSnapshot();
  const main = snapshot.project.timeline.main;
  main[1] = { ...main[1], ...patch };
  return snapshot;
}

/** jsdom has no layout or pointer capture: `slider`'s track is 100 px wide, and captured for the press. */
function track(slider: HTMLElement) {
  const root = slider.parentElement!.parentElement!;
  root.getBoundingClientRect = () => ({ left: 0, width: 100, top: 0, height: 20 }) as DOMRect;
  Object.assign(Element.prototype, {
    setPointerCapture: () => undefined,
    releasePointerCapture: () => undefined,
    hasPointerCapture: () => true,
  });
  return root;
}

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useSelection.setState({ selectedId: 'b' });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
});

describe('the clip card', () => {
  it('shows the selected clip by its file name, framed as Rust draws it', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('complementary', { name: 'broll.mp4 settings' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'broll.mp4' })).toBeInTheDocument();
    // An imported clip fills from the centre: Rust's answer, asked for this clip.
    expect(await screen.findByRole('radio', { name: 'Fill' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('radio', { name: 'Fit' })).toHaveAttribute('aria-checked', 'false');
    expect(clipFraming).toHaveBeenCalledWith('b');
  });

  it('Fit and Fill set the format shown only, built from the framing Rust draws, and keep the other formats', async () => {
    useProject.setState({
      snapshot: withB({ framing: { portrait: { mode: 'fill', focusX: 0.2, focusY: 0.5 } } }),
    });
    renderWithProviders(<Inspector format="landscape" />);
    await screen.findByRole('radio', { name: 'Fill', checked: true });
    await userEvent.click(screen.getByRole('radio', { name: 'Fit' }));
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith(
        'clip.framing.fit.undo',
        [
          {
            type: 'updateClip',
            id: 'b',
            patch: {
              framing: {
                portrait: { mode: 'fill', focusX: 0.2, focusY: 0.5 },
                landscape: { mode: 'fit', focusX: 0.5, focusY: 0.5 },
              },
            },
          },
        ],
        undefined,
        queued,
      ),
    );
  });

  it('a recording that follows the cursor keeps following when it fills again, and has no focus to set', async () => {
    useSelection.setState({ selectedId: 'a' });
    renderWithProviders(<Inspector format="portrait" />);
    expect(await screen.findByRole('radio', { name: 'Fill' })).toHaveAttribute('aria-checked', 'true');
    // The crop follows the cursor, so there is no focus to set.
    expect(screen.queryByRole('slider', { name: 'Focus across' })).toBeNull();
    await userEvent.click(screen.getByRole('radio', { name: 'Fit' }));
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith(
        'clip.framing.fit.undo',
        [{ type: 'updateClip', id: 'a', patch: { framing: { portrait: { ...follows, mode: 'fit' } } } }],
        undefined,
        queued,
      ),
    );
  });

  it('the Fill focus shows only with Fill; a drag dry-runs in the engine and commits once on release', async () => {
    useProject.setState({
      snapshot: withB({ framing: { square: { mode: 'fit', focusX: 0.5, focusY: 0.5 } } }),
    });
    const { unmount } = renderWithProviders(<Inspector format="square" />);
    expect(await screen.findByRole('radio', { name: 'Fit', checked: true })).toBeInTheDocument();
    expect(screen.queryByRole('slider', { name: 'Focus across' })).toBeNull();
    unmount();
    renderWithProviders(<Inspector format="portrait" />);
    await screen.findByRole('radio', { name: 'Fill', checked: true });
    expect(screen.getByRole('slider', { name: 'Focus down' })).toHaveAttribute('aria-valuetext', '50%');
    const across = screen.getByRole('slider', { name: 'Focus across' });
    const root = track(across);
    fireEvent.pointerDown(root, { button: 0, pointerId: 1, clientX: 60 });
    fireEvent.pointerMove(root, { pointerId: 1, clientX: 30 });
    expect(across).toHaveAttribute('aria-valuetext', '30%');
    await waitFor(() => expect(engine.preview).toHaveBeenCalled());
    expect(apply).not.toHaveBeenCalled();
    fireEvent.pointerUp(root, { pointerId: 1, clientX: 30 });
    expect(apply).toHaveBeenCalledTimes(1);
    const [label, ops] = apply.mock.calls[0];
    expect(label).toBe('clip.framing.focus.undo');
    expect(ops).toEqual([
      {
        type: 'updateClip',
        id: 'b',
        patch: {
          framing: {
            square: { mode: 'fit', focusX: 0.5, focusY: 0.5 },
            portrait: { mode: 'fill', focusX: 0.3, focusY: 0.5 },
          },
        },
      },
    ]);
    expect(vi.mocked(engine.preview).mock.calls.at(-1)).toEqual(['project', 3, ops]);
  });

  it('its more menu duplicates or deletes the clip, and has no layer order; Close lets it go', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'More' }));
    expect(screen.getByRole('menuitem', { name: 'Duplicate' })).toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'Bring forward' })).toBeNull();
    await userEvent.click(screen.getByRole('menuitem', { name: 'Delete' }));
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith('undo.delete.clip', [{ type: 'removeClip', id: 'b' }], undefined),
    );
    await userEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(useSelection.getState().selectedId).toBeNull();
  });

  it('Escape outside a field closes it', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    act(() => screen.getByRole('button', { name: 'More' }).focus());
    fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    expect(useSelection.getState().selectedId).toBeNull();
  });
});
