import { fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Clip } from '../../bindings/Clip';
import type { ClipFraming } from '../../bindings/ClipFraming';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The clip card's crop: the edges cut off its footage, in every format, each edge one labelled step on release, built
 * at its turn on the crop as the project has it then.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
/** Rust's framing: clip a (a screen recording) is fitted in 16:9, clip b (an imported video) fills. */
const fit = { mode: 'fit', focusX: 0.5, focusY: 0.5 } as const;
const fill = { mode: 'fill', focusX: 0.5, focusY: 0.5 } as const;
const WIDE = {
  landscape: { x: 0, y: 0, w: 1, h: 1 },
  portrait: { x: 0.341796875, y: 0, w: 0.31640625, h: 1 },
  square: { x: 0.21875, y: 0, w: 0.5625, h: 1 },
};
const FRAMING: Record<string, ClipFraming> = {
  a: { framing: { landscape: fit, portrait: fit, square: fit }, follows: false, speaker: false, fillCrop: WIDE },
  b: { framing: { landscape: fill, portrait: fill, square: fill }, follows: false, speaker: false, fillCrop: WIDE },
};
vi.mock('../../bridge/framing', () => ({
  clipFraming: vi.fn((clipId: string) => Promise.resolve(FRAMING[clipId])),
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Inspector } = await import('./Inspector');

const queued = expect.objectContaining({ projectId: 'project', revision: 3 });

/** The fixture with clip b changed by `clip`. */
function withB(clip: Partial<Clip>) {
  const snapshot = fixtureSnapshot();
  snapshot.project.timeline.main[1] = { ...snapshot.project.timeline.main[1], ...clip };
  useProject.setState({ snapshot, pending: 0, apply });
}

/** The slider named `name` on a 100 px track, pressed and let go at `x` px (jsdom has no layout or capture). */
function dragSlider(name: string, x: number) {
  const thumb = screen.getByRole('slider', { name });
  const root = thumb.parentElement!.parentElement!;
  root.getBoundingClientRect = () => ({ left: 0, width: 100, top: 0, height: 20 }) as DOMRect;
  Object.assign(Element.prototype, {
    setPointerCapture: () => undefined,
    releasePointerCapture: () => undefined,
    hasPointerCapture: () => true,
  });
  fireEvent.pointerDown(root, { button: 0, pointerId: 1, clientX: x });
  fireEvent.pointerUp(root, { pointerId: 1, clientX: x });
}

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useSelection.setState({ selectedId: 'b' });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
});

describe("the clip card's crop", () => {
  it('opens its edges, each one step on release, keeping the others', async () => {
    renderWithProviders(<Inspector format="portrait" />);
    const toggle = await screen.findByRole('button', { name: 'Crop: None' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('slider', { name: 'Crop left' })).toBeNull();
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    dragSlider('Crop left', 20);
    expect(apply).toHaveBeenLastCalledWith(
      'crop.undo',
      [{ type: 'updateClip', id: 'b', patch: { crop: { left: 0.18, top: 0, right: 0, bottom: 0 } } }],
      undefined,
      queued,
    );
    // No Reset until it is cropped.
    expect(screen.queryByRole('button', { name: 'Reset crop' })).toBeNull();
  });

  it('a cropped clip opens with its edges, holds each edge so a tenth is left, and resets', async () => {
    withB({ crop: { left: 0.2, top: 0, right: 0.5, bottom: 0.1 } });
    renderWithProviders(<Inspector format="landscape" />);
    expect(await screen.findByRole('button', { name: 'Crop: Cropped' })).toHaveAttribute('aria-expanded', 'true');
    const left = screen.getByRole('slider', { name: 'Crop left' });
    expect(left).toHaveAttribute('aria-valuetext', '20%');
    // Every edge's slider spans the same range, so a knob never jumps when the other edge moves; what goes in is held
    // so a tenth stays: right goes to 70 % at most while left cuts 20 %.
    expect(left).toHaveAttribute('aria-valuemax', '0.9');
    expect(screen.getByRole('slider', { name: 'Crop right' })).toHaveAttribute('aria-valuemax', '0.9');
    dragSlider('Crop right', 100);
    expect(apply).toHaveBeenLastCalledWith(
      'crop.undo',
      [{ type: 'updateClip', id: 'b', patch: { crop: { left: 0.2, top: 0, right: 0.7, bottom: 0.1 } } }],
      undefined,
      queued,
    );
    await userEvent.click(screen.getByRole('button', { name: 'Reset crop' }));
    await waitFor(() =>
      expect(apply).toHaveBeenLastCalledWith(
        'crop.reset.undo',
        [{ type: 'updateClip', id: 'b', patch: { crop: { left: 0, top: 0, right: 0, bottom: 0 } } }],
        undefined,
        queued,
      ),
    );
  });

  it('an edge whose opposite cuts 90% still has a slider that works, holding it at none', async () => {
    withB({ crop: { left: 0, top: 0, right: 0.9, bottom: 0 } });
    renderWithProviders(<Inspector format="landscape" />);
    const toggle = await screen.findByRole('button', { name: 'Crop: Cropped' });
    expect(toggle).toHaveAttribute('aria-controls');
    expect(document.getElementById(toggle.getAttribute('aria-controls')!)).toContainElement(
      screen.getByRole('slider', { name: 'Crop left' }),
    );
    expect(screen.getByRole('slider', { name: 'Crop left' })).toHaveAttribute('aria-valuemax', '0.9');
    dragSlider('Crop left', 50);
    // Held at none, the drag changes nothing: no step goes in.
    expect(apply.mock.calls.every(([, ops]) => ops.length === 0)).toBe(true);
  });
});
