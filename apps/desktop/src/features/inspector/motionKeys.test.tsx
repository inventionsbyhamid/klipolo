import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Clip } from '../../bindings/Clip';
import type { Key } from '../../bindings/Key';
import type { Layout } from '../../bindings/Layout';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { renderCounts } from '../../lib/renderCount';
import { FORMAT_SIZE } from '../../lib/layout';
import { positionedLayout } from './inspectorOps';

/*
 * Keys set by hand in the cards: a layer's Motion rows (a key at the playhead holding the value there, its ease, and
 * the keys before and after it), and Add volume key on a clip, whose Volume then sets the key at the playhead.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn(() => Promise.resolve()),
    previewClear: vi.fn(() => Promise.resolve()),
    seek: vi.fn(() => Promise.resolve()),
  },
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Inspector } = await import('./Inspector');

const S = 1_000_000;
const key = (t: number, v: number): Key => ({ tUs: t * S, v });

/** The fixture's title (1 s to 4 s, fading in and out) selected, with `keys` on its motion. */
function title(keys?: Partial<NonNullable<NonNullable<Layout['motion']>['keys']>>) {
  const snapshot = fixtureSnapshot();
  const item = snapshot.project.timeline.tracks[0].items[0];
  if (keys)
    item.layout.motion = {
      ...item.layout.motion,
      keys: { x: [], y: [], scale: [], rotation: [], opacity: [], ...keys },
    };
  useProject.setState({ snapshot, pending: 0, apply });
  useSelection.setState({ selectedId: 'title', selectedIds: ['title'] });
  return snapshot;
}

const layoutOf = (call: Parameters<typeof apply>) =>
  (call[1][0] as unknown as { patch: { layout: Layout } }).patch.layout;

beforeEach(() => {
  vi.clearAllMocks();
  usePlayback.setState({ playheadUs: 2 * S, playing: false });
});

describe('Motion keys in the card', () => {
  it('a key button adds a key at the playhead holding the value there, and keeps the enter and exit', async () => {
    title();
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Add position key' }));
    expect(apply).toHaveBeenCalledTimes(1);
    const [label, , params] = apply.mock.calls[0];
    expect(label).toBe('keys.add.undo');
    expect(params).toEqual({ property: 'position' });
    const motion = layoutOf(apply.mock.calls[0]).motion!;
    // 1 s into the title, at rest: no offset.
    expect(motion.keys?.x).toEqual([key(1, 0)]);
    expect(motion.keys?.y).toEqual([key(1, 0)]);
    expect(motion.enter && motion.exit).toBeTruthy();
  });

  it('a key at the playhead shows its ease, which the card changes, and its button takes it away', async () => {
    title({ scale: [key(0, 1), key(1, 2)] });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Scale ease: Linear' }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Spring' }));
    expect(apply.mock.calls[0][0]).toBe('keys.ease.undo');
    expect(layoutOf(apply.mock.calls[0]).motion?.keys?.scale[1].ease).toEqual({
      spring: { stiffness: 100, damping: 10, mass: 1 },
    });
    await userEvent.click(screen.getByRole('button', { name: 'Remove scale key' }));
    expect(apply.mock.calls[1][0]).toBe('keys.remove.undo');
    expect(layoutOf(apply.mock.calls[1]).motion?.keys?.scale).toEqual([key(0, 1)]);
  });

  it('with position keys, the Position grid sets the key at the playhead and leaves the layer where it rests', async () => {
    const snapshot = title({ x: [key(0, 0), key(2, 0.2)] });
    const rest = snapshot.project.timeline.tracks[0].items[0].layout;
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('radio', { name: 'Top left' }));
    expect(apply.mock.calls[0][0]).toBe('layout.position.undo');
    const layout = layoutOf(apply.mock.calls[0]);
    expect(layout.rect).toEqual(rest.rect);
    // Drawn at top left: its own box moved by the keys' offset at the playhead (1 s into the title).
    const want = positionedLayout(rest, 'topLeft', 'landscape', 'landscape', FORMAT_SIZE.landscape).rect;
    const x = layout.motion!.keys!.x.find((k) => k.tUs === S)!.v;
    const y = layout.motion!.keys!.y.find((k) => k.tUs === S)!.v;
    expect(rest.rect.x + x).toBeCloseTo(want.x, 9);
    expect(rest.rect.y + y).toBeCloseTo(want.y, 9);
  });

  it('an adjustment layer, which stays upright, has no Rotation row', () => {
    const snapshot = title();
    const item = snapshot.project.timeline.tracks[0].items[0];
    item.content = { type: 'adjustment' };
    item.layout.motion = undefined;
    useProject.setState({ snapshot, pending: 0, apply });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('button', { name: 'Add scale key' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Add rotation key' })).toBeNull();
  });

  it('the keys before and after the playhead put the playhead on them', async () => {
    title({ opacity: [key(0.5, 1), key(2.5, 0)] });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('button', { name: 'Previous opacity key' })).toBeEnabled();
    await userEvent.click(screen.getByRole('button', { name: 'Next opacity key' }));
    expect(usePlayback.getState().playheadUs).toBe(3.5 * S);
    expect(screen.getByRole('button', { name: 'Next opacity key' })).toBeDisabled();
  });
});

describe('the cards during playback', () => {
  it('re-render only where what they show changes, not on every frame between keys', () => {
    title({ scale: [key(0, 1), key(2, 2)] });
    renderWithProviders(<Inspector format="landscape" />);
    const motion = renderCounts.get('motion:title') ?? 0;
    act(() => usePlayback.setState({ playheadUs: 2.1 * S }));
    act(() => usePlayback.setState({ playheadUs: 2.2 * S }));
    expect(renderCounts.get('motion:title')).toBe(motion);
    // Onto the key at 2 s into the title: its button and ease show.
    act(() => usePlayback.setState({ playheadUs: 3 * S }));
    expect(renderCounts.get('motion:title')).toBeGreaterThan(motion);
  });

  it("a picture layer with keys re-renders its Size and Opacity only when what they show changes", () => {
    // The logo as an image layer from 1 s to 4 s, fading out over its last two seconds by keys.
    const snapshot = title();
    const [track] = snapshot.project.timeline.tracks;
    track.items[0] = {
      ...track.items[0],
      content: { type: 'image', assetId: 'logo' },
      layout: { ...track.items[0].layout, motion: { keys: { x: [], y: [], scale: [], rotation: [], opacity: [key(1, 1), key(3, 0)] } } },
    };
    renderWithProviders(<Inspector format="landscape" />);
    renderCounts.clear();
    // Before the fade: what Size and Opacity show stays the same however the playhead moves.
    for (const at of [1.1, 1.3, 1.5, 1.7, 1.9]) act(() => usePlayback.setState({ playheadUs: at * S }));
    expect(renderCounts.get('picture:title') ?? 0).toBe(0);
    // Into the fade: Opacity shows the new level.
    act(() => usePlayback.setState({ playheadUs: 3 * S }));
    expect(renderCounts.get('picture:title')).toBeGreaterThan(0);
  });

  it('Volume with keys re-renders when the level it shows changes', () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.main[1].volumeKeys = [key(0, 1), key(4, 0)];
    useProject.setState({ snapshot, pending: 0, apply });
    useSelection.setState({ selectedId: 'b', selectedIds: ['b'] });
    usePlayback.setState({ playheadUs: 21 * S });
    renderWithProviders(<Inspector format="landscape" />);
    const volume = renderCounts.get('volume:b') ?? 0;
    act(() => usePlayback.setState({ playheadUs: 21 * S + 1_000 }));
    expect(renderCounts.get('volume:b')).toBe(volume);
    act(() => usePlayback.setState({ playheadUs: 22 * S }));
    expect(renderCounts.get('volume:b')).toBeGreaterThan(volume);
  });
});

describe('Volume keys in the clip card', () => {
  /** Clip "b" (the b-roll at 20 s) selected, with `keys`. */
  function clip(keys?: Key[]) {
    const snapshot = fixtureSnapshot();
    const b = snapshot.project.timeline.main[1] as Clip;
    if (keys) b.volumeKeys = keys;
    useProject.setState({ snapshot, pending: 0, apply });
    useSelection.setState({ selectedId: 'b', selectedIds: ['b'] });
  }

  it('Add volume key puts a key at the playhead holding the level there', async () => {
    clip([key(0, 1), key(4, 0)]);
    usePlayback.setState({ playheadUs: 21 * S });
    renderWithProviders(<Inspector format="landscape" />);
    // With keys, Volume shows the level at the playhead: 75 % a second in.
    expect(screen.getByRole('slider', { name: 'Volume' })).toHaveAttribute('aria-valuetext', '75%');
    await userEvent.click(screen.getByRole('button', { name: 'Add volume key' }));
    expect(apply.mock.calls[0][0]).toBe('volumeKeys.add.undo');
    const op = apply.mock.calls[0][1][0] as Extract<Op, { type: 'updateClip' }>;
    expect(op.patch.volumeKeys).toEqual([key(0, 1), key(1, 0.75), key(4, 0)]);
  });

  it('with keys, Volume is off while the playhead is outside the clip: it has no key to set there', () => {
    clip([key(0, 1), key(4, 0)]);
    usePlayback.setState({ playheadUs: 5 * S });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('slider', { name: 'Volume' })).toHaveAttribute('aria-disabled', 'true');
  });

  it('the volume keys before and after the playhead put the playhead on them', async () => {
    clip([key(0.5, 1), key(4, 0)]);
    usePlayback.setState({ playheadUs: 22 * S });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Next volume key' }));
    expect(usePlayback.getState().playheadUs).toBe(24 * S);
    await userEvent.click(screen.getByRole('button', { name: 'Previous volume key' }));
    expect(usePlayback.getState().playheadUs).toBe(20.5 * S);
    expect(screen.getByRole('button', { name: 'Previous volume key' })).toBeDisabled();
  });

  it('a key at the playhead is taken away by the same button; the last one leaves the clip at its level', async () => {
    clip([key(1, 0.5)]);
    usePlayback.setState({ playheadUs: 21 * S });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Remove volume key' }));
    expect(apply.mock.calls[0][0]).toBe('volumeKeys.remove.undo');
    const { patch } = apply.mock.calls[0][1][0] as Extract<Op, { type: 'updateClip' }>;
    expect(patch.volumeKeys).toEqual([]);
    // Not back to a volume from before the keys: it stays where its line was.
    expect(patch.volume).toBe(0.5);
  });
});
