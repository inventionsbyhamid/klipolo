import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Background } from '../../bindings/Background';
import type { Clip } from '../../bindings/Clip';
import type { ClipFraming } from '../../bindings/ClipFraming';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The clip card's background: under Fit, what shows behind the video where it leaves the frame uncovered (the blurred
 * video, black, a color, a gradient or a picture) and the padding round it. Each change is one labelled step built at
 * its turn through the commit queue.
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
  a: { framing: { landscape: fit, portrait: fit, square: fit }, follows: false, fillCrop: WIDE },
  b: { framing: { landscape: fill, portrait: fill, square: fill }, follows: false, fillCrop: WIDE },
};
vi.mock('../../bridge/framing', () => ({
  clipFraming: vi.fn((clipId: string) => Promise.resolve(FRAMING[clipId])),
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Inspector } = await import('./Inspector');

const queued = expect.objectContaining({ projectId: 'project', revision: 3 });
const BLACK: Background = { kind: 'none', color: '#101114', colorEnd: null, assetId: null, padding: 0 };

/** The fixture with its background `patch`ed and clip b changed by `clip`. */
function withProject(background: Partial<Background>, clip: Partial<Clip> = {}) {
  const snapshot = fixtureSnapshot();
  snapshot.project.background = { ...snapshot.project.background, ...background };
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

/** The background `apply` was last asked to set, with its label. */
function setTo(label: string, background: Background) {
  expect(apply).toHaveBeenLastCalledWith(label, [{ type: 'setBackground', background }], undefined, queued);
}

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useSelection.setState({ selectedId: 'a' });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
});

describe("the clip card's background", () => {
  it('shows under Fit only, with the padding, until the background has padding', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    await screen.findByRole('radio', { name: 'Fit', checked: true });
    expect(screen.getByRole('button', { name: 'Background: Black' })).toBeInTheDocument();
    expect(screen.getByRole('slider', { name: 'Padding' })).toHaveAttribute('aria-valuetext', '0%');
    useSelection.setState({ selectedId: 'b' });
    await screen.findByRole('radio', { name: 'Fill', checked: true });
    expect(screen.queryByRole('button', { name: /^Background/ })).toBeNull();
    // Padding shows the background round a filled clip too.
    withProject({ padding: 0.1 });
    expect(await screen.findByRole('slider', { name: 'Padding' })).toHaveAttribute('aria-valuetext', '10%');
  });

  it('puts the blurred video behind it in one step, keeping the rest', async () => {
    withProject({ padding: 0.05 });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(await screen.findByRole('button', { name: 'Background: Black' }));
    expect(screen.getAllByRole('menuitemradio').map((item) => item.textContent)).toEqual([
      'Blurred video',
      'Black',
      'Color',
      'Gradient',
      'Picture',
    ]);
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Blurred video' }));
    await waitFor(() => setTo('background.kind.undo', { ...BLACK, kind: 'blur', padding: 0.05 }));
  });

  it('a color, a gradient and a picture each pick theirs', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(await screen.findByRole('button', { name: 'Background: Black' }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Gradient' }));
    // A gradient gets a second color.
    await waitFor(() => setTo('background.kind.undo', { ...BLACK, kind: 'gradient', colorEnd: '#7F92DA' }));
    await userEvent.click(screen.getByRole('button', { name: 'Background: Black' }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Picture' }));
    // The project's first image.
    await waitFor(() => setTo('background.kind.undo', { ...BLACK, kind: 'image', assetId: 'logo' }));

    withProject({ kind: 'solid' });
    const color = await screen.findByRole('radiogroup', { name: 'Background color' });
    await userEvent.click(within(color).getByRole('radio', { name: 'Yellow' }));
    await waitFor(() => setTo('background.color.undo', { ...BLACK, kind: 'solid', color: '#FFD02E' }));

    withProject({ kind: 'gradient', color: '#FFFFFF', colorEnd: '#121A33' });
    const end = await screen.findByRole('radiogroup', { name: 'Gradient end color' });
    expect(within(end).getByRole('radio', { name: 'Navy' })).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(within(end).getByRole('radio', { name: 'Black' }));
    await waitFor(() =>
      setTo('background.color.undo', { ...BLACK, kind: 'gradient', color: '#FFFFFF', colorEnd: '#000000' }),
    );

    withProject({ kind: 'image', assetId: 'logo' });
    expect(await screen.findByRole('button', { name: 'Background picture: logo.png' })).toBeInTheDocument();
  });

  it('no picture is offered in a project without images', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.assets = snapshot.project.assets.filter((a) => a.kind !== 'image');
    useProject.setState({ snapshot, pending: 0, apply });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(await screen.findByRole('button', { name: 'Background: Black' }));
    expect(screen.queryByRole('menuitemradio', { name: 'Picture' })).toBeNull();
  });

  it('a padding drag is one step on release', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    await screen.findByRole('radio', { name: 'Fit', checked: true });
    dragSlider('Padding', 25);
    expect(apply).toHaveBeenCalledTimes(1);
    setTo('background.padding.undo', { ...BLACK, padding: 0.1 });
  });
});
