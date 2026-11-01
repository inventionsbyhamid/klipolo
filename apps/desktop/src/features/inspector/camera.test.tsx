import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * A recording's camera: Bubble, Split (9:16) or Hidden in the format shown, and as a bubble its shape and border,
 * mirrored wherever it shows. The Camera card has them while the bubble is selected on the video, and the clip card of
 * a recording clip has them too (the way back to a camera hidden there). Each change is one labelled step, built at its
 * turn through the commit queue.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
vi.mock('../../bridge/framing', () => {
  const fit = { mode: 'fit', focusX: 0.5, focusY: 0.5 };
  const whole = { x: 0, y: 0, w: 1, h: 1 };
  return {
    clipFraming: vi.fn(() =>
      Promise.resolve({
        framing: { landscape: fit, portrait: fit, square: fit },
        follows: false,
        fillCrop: { landscape: whole, portrait: whole, square: whole },
      }),
    ),
  };
});
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Inspector } = await import('./Inspector');

/** Where a change through the commit queue was asked for (`useProject.apply`'s `built`). */
const queued = expect.objectContaining({ projectId: 'project', revision: 3 });
const camera = () => fixtureSnapshot().project.timeline.main[0].camera!;

/** The one change sent to clip a's camera: `label`, and the camera with `fields` set and the rest as it was. */
function sentCamera(label: string, fields: object) {
  expect(apply).toHaveBeenCalledTimes(1);
  expect(apply).toHaveBeenCalledWith(
    label,
    [{ type: 'updateClip', id: 'a', patch: { camera: { ...camera(), ...fields } } }],
    undefined,
    queued,
  );
}

/** The radio group named `name`. */
const group = (name: string) => screen.getByRole('radiogroup', { name });

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useSelection.setState({ selectedId: 'camera:a' });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
});

describe('the Camera card', () => {
  it('shows while the bubble is selected: its layout, shape, border and mirror in the format shown', () => {
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('complementary', { name: 'Camera settings' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Camera' })).toBeInTheDocument();
    // Split suits 9:16 only.
    expect(within(group('Camera layout')).getAllByRole('radio').map((r) => r.textContent)).toEqual([
      'Bubble',
      'Hidden',
    ]);
    expect(within(group('Camera layout')).getByRole('radio', { name: 'Bubble' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(within(group('Camera shape')).getByRole('radio', { name: 'Circle' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(within(group('Border')).getByRole('radio', { name: 'On' })).toHaveAttribute('aria-checked', 'true');
    expect(within(group('Mirror')).getByRole('radio', { name: 'Off' })).toHaveAttribute('aria-checked', 'true');
  });

  it('Hidden in 9:16 sets the 9:16 layout only', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.settings.formats = ['landscape', 'portrait'];
    useProject.setState({ snapshot });
    renderWithProviders(<Inspector format="portrait" />);
    expect(within(group('Camera layout')).getByRole('radio', { name: 'Split' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    await userEvent.click(within(group('Camera layout')).getByRole('radio', { name: 'Hidden' }));
    sentCamera('camera.layout.undo', { mode: 'bubble', modeOverrides: { portrait: 'hidden' } });
  });

  it('a layout change sets the format shown only, even the main one', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.settings = {
      ...snapshot.project.settings,
      formats: ['portrait', 'landscape', 'square'],
      primaryFormat: 'portrait',
    };
    snapshot.project.timeline.main[0].camera!.modeOverrides = {};
    useProject.setState({ snapshot });
    renderWithProviders(<Inspector format="portrait" />);
    await userEvent.click(within(group('Camera layout')).getByRole('radio', { name: 'Split' }));
    expect(apply).toHaveBeenCalledWith(
      'camera.layout.undo',
      [
        {
          type: 'updateClip',
          id: 'a',
          patch: { camera: { ...camera(), mode: 'bubble', modeOverrides: { portrait: 'splitBottom' } } },
        },
      ],
      undefined,
      queued,
    );
  });

  it('Shape, Border and Mirror are one step each, setting only their own field', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(within(group('Camera shape')).getByRole('radio', { name: 'Rounded' }));
    sentCamera('camera.shape.undo', { shape: 'rounded' });
    apply.mockClear();
    await userEvent.click(within(group('Border')).getByRole('radio', { name: 'Off' }));
    sentCamera('camera.border.hide.undo', { border: false });
    apply.mockClear();
    await userEvent.click(within(group('Mirror')).getByRole('radio', { name: 'On' }));
    sentCamera('camera.mirror.undo', { mirror: true });
  });

  it('as the split it has no shape or border, and hidden only its layout', () => {
    const { unmount } = renderWithProviders(<Inspector format="portrait" />);
    expect(screen.queryByRole('radiogroup', { name: 'Camera shape' })).toBeNull();
    expect(screen.queryByRole('radiogroup', { name: 'Border' })).toBeNull();
    expect(group('Mirror')).toBeInTheDocument();
    unmount();

    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.main[0].camera!.mode = 'hidden';
    useProject.setState({ snapshot });
    renderWithProviders(<Inspector format="landscape" />);
    expect(group('Camera layout')).toBeInTheDocument();
    for (const name of ['Camera shape', 'Border', 'Mirror'])
      expect(screen.queryByRole('radiogroup', { name })).toBeNull();
  });

  it('closes with its close button', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(useSelection.getState().selectedId).toBeNull();
  });
});

/** Clip a's camera separated into a layer of its own (`a-camera`): the clip's own camera is hidden everywhere. */
function separated() {
  const snapshot = fixtureSnapshot();
  const layer: Item = {
    ...titleItem,
    id: 'a-camera',
    startUs: 200_000,
    durationUs: 19_800_000,
    layout: { ...titleItem.layout, rect: camera().bubble.rect, motion: undefined },
    attach: { clipId: 'a', source: { type: 'recording', id: 'rec' }, sourceUs: 200_000 },
    content: { type: 'video', assetId: 'camera', sourceInUs: 0, speed: 1, volume: 1, muted: true },
  };
  snapshot.project.timeline.tracks[0].items.push(layer);
  snapshot.project.timeline.main[0].camera = {
    ...camera(),
    mode: 'hidden',
    modeOverrides: { portrait: 'hidden' },
  };
  return snapshot;
}

describe("a recording clip's card", () => {
  beforeEach(() => useSelection.setState({ selectedId: 'a' }));

  it("has the camera's controls: a camera hidden here comes back from it", async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.main[0].camera!.modeOverrides = { portrait: 'splitBottom', landscape: 'hidden' };
    useProject.setState({ snapshot });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('complementary', { name: 'Recording 1 settings' })).toBeInTheDocument();
    await userEvent.click(within(group('Camera layout')).getByRole('radio', { name: 'Bubble' }));
    expect(apply).toHaveBeenCalledWith(
      'camera.layout.undo',
      [
        {
          type: 'updateClip',
          id: 'a',
          patch: { camera: { ...camera(), modeOverrides: { portrait: 'splitBottom', landscape: 'bubble' } } },
        },
      ],
      undefined,
      queued,
    );
  });

  it('once the camera is its own layer, goes to that layer instead', async () => {
    useProject.setState({ snapshot: separated() });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.queryByRole('radiogroup', { name: 'Camera layout' })).toBeNull();
    expect(screen.getByText('The camera is its own layer now.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Select camera layer' }));
    expect(useSelection.getState().selectedId).toBe('a-camera');
  });

  it('of a take of the camera alone mirrors it, with no layout to pick', async () => {
    const snapshot = fixtureSnapshot();
    const rec = snapshot.project.recordings[0];
    rec.sources = rec.sources.filter((s) => s.id !== 'screen');
    rec.primary = 'camera';
    rec.kind = 'camera';
    useProject.setState({ snapshot });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.queryByRole('radiogroup', { name: 'Camera layout' })).toBeNull();
    await userEvent.click(within(group('Mirror')).getByRole('radio', { name: 'On' }));
    sentCamera('camera.mirror.undo', { mirror: true });
  });

  it('of an imported video has none of it', () => {
    useSelection.setState({ selectedId: 'b' });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('complementary', { name: 'broll.mp4 settings' })).toBeInTheDocument();
    for (const name of ['Camera layout', 'Mirror']) expect(screen.queryByRole('radiogroup', { name })).toBeNull();
  });
});
