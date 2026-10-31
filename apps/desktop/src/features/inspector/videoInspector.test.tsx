import { fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * A video layer's card (a picture in picture): its picture fields, its sound (Volume and Mute), Speed and Mirror, each
 * one labelled change built at its turn through the commit queue, and its order among the layers from the more menu.
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

/** Where a change through the commit queue was asked for (`useProject.apply`'s `built`). */
const queued = expect.objectContaining({ projectId: 'project', revision: 3 });

/** A video layer over the clip: `a-camera` is clip a's separated camera, any other id the b-roll (which has sound). */
function videoLayer(id: string, mirror?: boolean): Item {
  const camera = id === 'a-camera';
  return {
    ...titleItem,
    id,
    startUs: 1_000_000,
    layout: { ...titleItem.layout, rect: { x: 0.56, y: 0.56, w: 0.4, h: 0.4 }, motion: undefined },
    attach: camera ? { clipId: 'a', source: { type: 'recording', id: 'rec' }, sourceUs: 1_000_000 } : null,
    content: {
      type: 'video',
      assetId: camera ? 'camera' : 'broll',
      sourceInUs: 0,
      speed: 1,
      volume: 1,
      muted: false,
      ...(mirror === undefined ? {} : { mirror }),
    },
  };
}

/** The fixture with video layer `id` on the overlay track, selected. */
function withVideo(id: string, mirror?: boolean) {
  const snapshot = fixtureSnapshot();
  snapshot.project.timeline.tracks[0].items.push(videoLayer(id, mirror));
  useProject.setState({ snapshot, pending: 0, apply });
  useSelection.setState({ selectedId: id });
  return snapshot;
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

describe("a video layer's card", () => {
  it('shows Size, Opacity, Corners, Volume and Mute, Speed, Mirror, Position and Timing', () => {
    withVideo('v1');
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('complementary', { name: 'Video settings' })).toBeInTheDocument();
    expect(screen.getByRole('slider', { name: 'Size' })).toHaveAttribute('aria-valuetext', '40%');
    expect(screen.getByRole('slider', { name: 'Opacity' })).toHaveAttribute('aria-valuetext', '100%');
    expect(screen.getByRole('slider', { name: 'Corner radius' })).toBeInTheDocument();
    expect(screen.getByRole('slider', { name: 'Volume' })).toHaveAttribute('aria-valuetext', '100%');
    expect(screen.getByRole('button', { name: 'Mute' })).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByRole('button', { name: 'Speed: 1×' })).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: 'Off' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('radio', { name: 'Bottom right' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('textbox', { name: 'Starts' })).toHaveValue('0:01.0');
    expect(screen.getByRole('textbox', { name: 'Lasts' })).toHaveValue('3.0 s');
  });

  it('Volume is one change on release, and Mute switches the video’s own sound, keeping its volume', async () => {
    withVideo('v1');
    renderWithProviders(<Inspector format="landscape" />);
    dragSlider('Volume', 30);
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply).toHaveBeenCalledWith(
      'volume.undo',
      [{ type: 'updateItem', id: 'v1', patch: { content: expect.objectContaining({ volume: 0.6 }) } }],
      undefined,
      queued,
    );
    await userEvent.click(screen.getByRole('button', { name: 'Mute' }));
    expect(apply).toHaveBeenLastCalledWith(
      'volume.mute.undo',
      [{ type: 'updateItem', id: 'v1', patch: { content: expect.objectContaining({ muted: true, volume: 1 }) } }],
      { noun: 'video' },
      queued,
    );
  });

  it('Size is one change, named for the layer as a resize on the canvas is', async () => {
    withVideo('v1');
    const { unmount } = renderWithProviders(<Inspector format="landscape" />);
    dragSlider('Size', 50);
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toBe('gizmo.resize.undo');
    expect(apply.mock.calls[0][2]).toEqual({ layer: 'video' });
    unmount();
    // A picture's says "Resize image".
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.tracks[0].items.push({
      ...videoLayer('pic'),
      content: { type: 'image', assetId: 'logo' },
    });
    useProject.setState({ snapshot, pending: 0, apply });
    useSelection.setState({ selectedId: 'pic' });
    renderWithProviders(<Inspector format="landscape" />);
    dragSlider('Size', 50);
    // It goes in once the first has landed (the commit queue).
    await vi.waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
    expect(apply.mock.calls[1].slice(0, 3)).toEqual(['gizmo.resize.undo', expect.any(Array), { layer: 'image' }]);
  });

  it('a video whose file has no sound has no Volume or Mute', () => {
    const snapshot = withVideo('v1');
    snapshot.project.assets.find((a) => a.id === 'broll')!.hasAudio = false;
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.queryByRole('slider', { name: 'Volume' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Mute' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Speed: 1×' })).toBeInTheDocument();
  });

  it('Speed sets the layer’s own speed', async () => {
    withVideo('v1');
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Speed: 1×' }));
    expect(screen.getAllByRole('menuitemradio').map((option) => option.textContent)).toEqual([
      '0.5×',
      '0.75×',
      '1×',
      '1.25×',
      '1.5×',
      '2×',
    ]);
    await userEvent.click(screen.getByRole('menuitemradio', { name: '2×' }));
    expect(apply).toHaveBeenCalledWith(
      'speed.undo',
      [{ type: 'updateItem', id: 'v1', patch: { content: expect.objectContaining({ speed: 2 }) } }],
      undefined,
      queued,
    );
  });

  it('a speed set from chat is shown as it is, among the choices', async () => {
    const snapshot = withVideo('v1');
    const layer = snapshot.project.timeline.tracks[0].items.at(-1)!;
    layer.content = { ...(layer.content as Extract<Item['content'], { type: 'video' }>), speed: 1.1 };
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Speed: 1.1×' }));
    expect(screen.getByRole('menuitemradio', { name: '1.1×' })).toHaveAttribute('aria-checked', 'true');
  });

  it("a separated camera's speed is its clip's, so it stays in sync", async () => {
    withVideo('a-camera');
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByText("Changes the clip's speed too, so the camera stays in sync.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Speed: 1×' }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: '2×' }));
    expect(apply).toHaveBeenCalledWith(
      'speed.undo',
      [{ type: 'updateClip', id: 'a', patch: { speed: 2 } }],
      undefined,
      queued,
    );
  });

  it('Mirror flips any video layer as a video, and a separated camera under the camera’s undo labels', async () => {
    withVideo('v1');
    const { unmount } = renderWithProviders(<Inspector format="landscape" />);
    const mirror = screen.getByRole('radiogroup', { name: 'Mirror' });
    await userEvent.click(within(mirror).getByRole('radio', { name: 'On' }));
    expect(apply).toHaveBeenCalledWith(
      'media.mirror.undo',
      [{ type: 'updateItem', id: 'v1', patch: { content: expect.objectContaining({ mirror: true }) } }],
      undefined,
      queued,
    );
    unmount();
    withVideo('a-camera', true);
    renderWithProviders(<Inspector format="landscape" />);
    const camera = screen.getByRole('radiogroup', { name: 'Mirror' });
    expect(within(camera).getByRole('radio', { name: 'On' })).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(within(camera).getByRole('radio', { name: 'Off' }));
    expect(apply).toHaveBeenLastCalledWith(
      'camera.unmirror.undo',
      [{ type: 'updateItem', id: 'a-camera', patch: { content: expect.objectContaining({ mirror: false }) } }],
      undefined,
      queued,
    );
  });

  it('the more menu brings it forward or sends it back a track, and Duplicate and Delete are there', async () => {
    const snapshot = withVideo('v1');
    // An overlay track under the layer's and one over it.
    const { tracks } = snapshot.project.timeline;
    const empty = (id: string) => ({ ...tracks[0], id, name: id, items: [] });
    snapshot.project.timeline.tracks = [empty('below'), tracks[0], empty('above'), tracks[1]];
    renderWithProviders(<Inspector format="landscape" />);
    const more = () => userEvent.click(screen.getByRole('button', { name: 'More' }));
    await more();
    expect(screen.getAllByRole('menuitem').map((item) => item.textContent)).toEqual([
      'Duplicate',
      'Bring forward',
      'Send back',
      'Delete',
    ]);
    await userEvent.click(screen.getByRole('menuitem', { name: 'Send back' }));
    await vi.waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(apply.mock.calls[0].slice(0, 2)).toEqual([
      'undo.trackDown',
      [{ type: 'moveItem', id: 'v1', trackId: 'below', startUs: 1_000_000 }],
    ]);
    await more();
    await userEvent.click(screen.getByRole('menuitem', { name: 'Bring forward' }));
    await vi.waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
    expect(apply.mock.calls[1].slice(0, 2)).toEqual([
      'undo.trackUp',
      [{ type: 'moveItem', id: 'v1', trackId: 'above', startUs: 1_000_000 }],
    ]);
  });

  it('Bring forward and Send back are off while no track of its kind is there to take it', async () => {
    withVideo('v1');
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'More' }));
    expect(screen.getByRole('menuitem', { name: 'Bring forward' })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByRole('menuitem', { name: 'Send back' })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByRole('menuitem', { name: 'Duplicate' })).not.toHaveAttribute('aria-disabled');
  });
});
