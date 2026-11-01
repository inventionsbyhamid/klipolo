import { fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Crop } from '../../bindings/Crop';
import type { Item } from '../../bindings/Item';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * A video layer's picture inside its box: Fit (the whole picture) or Fill (covering the box, cutting the edges), and
 * its crop, each one labelled change built at its turn through the commit queue, keeping the rest of the layer.
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

const queued = expect.objectContaining({ projectId: 'project', revision: 3 });

/** The b-roll as a layer over the clip, filling its box or cropped as given, selected. */
function withVideo(fill?: boolean, crop?: Crop): Item {
  const layer: Item = {
    ...titleItem,
    id: 'v1',
    startUs: 1_000_000,
    layout: { ...titleItem.layout, rect: { x: 0.56, y: 0.56, w: 0.4, h: 0.4 }, motion: undefined },
    attach: null,
    content: {
      type: 'video',
      assetId: 'broll',
      sourceInUs: 0,
      speed: 1,
      volume: 1,
      muted: false,
      ...(fill === undefined ? {} : { fill }),
      ...(crop === undefined ? {} : { crop }),
    },
  };
  const snapshot = fixtureSnapshot();
  snapshot.project.timeline.tracks[0].items.push(layer);
  useProject.setState({ snapshot, pending: 0, apply });
  useSelection.setState({ selectedId: 'v1' });
  return layer;
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
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
});

describe("a video layer's picture in its box", () => {
  it('is fitted by default; Fill covers the box, one step keeping the rest', async () => {
    const layer = withVideo();
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('radio', { name: 'Fit' })).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(screen.getByRole('radio', { name: 'Fill' }));
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith(
        'clip.framing.fill.undo',
        [{ type: 'updateItem', id: 'v1', patch: { content: { ...layer.content, fill: true } } }],
        undefined,
        queued,
      ),
    );
  });

  it('a filled layer fits again', async () => {
    const layer = withVideo(true);
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('radio', { name: 'Fill' })).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(screen.getByRole('radio', { name: 'Fit' }));
    await waitFor(() =>
      expect(apply).toHaveBeenCalledWith(
        'clip.framing.fit.undo',
        [{ type: 'updateItem', id: 'v1', patch: { content: { ...layer.content, fill: false } } }],
        undefined,
        queued,
      ),
    );
  });

  it('crops an edge, and Reset takes the crop off', async () => {
    const layer = withVideo(undefined, { left: 0, top: 0.2, right: 0, bottom: 0 });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('button', { name: 'Crop: Cropped' })).toHaveAttribute('aria-expanded', 'true');
    dragSlider('Crop bottom', 50);
    expect(apply).toHaveBeenLastCalledWith(
      'crop.undo',
      [
        {
          type: 'updateItem',
          id: 'v1',
          patch: { content: { ...layer.content, crop: { left: 0, top: 0.2, right: 0, bottom: 0.35 } } },
        },
      ],
      undefined,
      queued,
    );
    await userEvent.click(screen.getByRole('button', { name: 'Reset crop' }));
    await waitFor(() => expect(apply).toHaveBeenLastCalledWith('crop.reset.undo', expect.any(Array), undefined, queued));
    const [, ops] = apply.mock.calls.at(-1)!;
    const op = ops[0] as Extract<Op, { type: 'updateItem' }>;
    expect(op.patch.content).toEqual({ ...layer.content, crop: undefined });
    expect(JSON.parse(JSON.stringify(op.patch.content))).not.toHaveProperty('crop');
  });
});
