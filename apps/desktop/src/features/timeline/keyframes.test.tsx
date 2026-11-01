import { act, fireEvent } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ItemContent } from '../../bindings/ItemContent';
import type { Key } from '../../bindings/Key';
import type { Layout } from '../../bindings/Layout';
import type { Op } from '../../bindings/Op';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * Keys set by hand on the timeline: a layer's motion keys as diamonds on its block (a click picks the layer and puts
 * the playhead there, a drag moves the keys, Option drag copies them, Delete takes them away, and Clear motion keys in
 * the layer's menu), and a selected clip's or sound's volume line (a click adds a key, a point drags, Delete removes).
 */

const apply = vi.fn<(label: string, ops: Op[]) => Promise<boolean>>(() => Promise.resolve(true));
vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { useProject } = await import('../../state/project');
const { useUi } = await import('../../state/ui');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { useSpeech } = await import('./media/speech');
const { Timeline } = await import('./Timeline');
const { clearMotionKeys } = await import('../../state/keyEdits');

type AudioContent = Extract<ItemContent, { type: 'audio' }>;
const S = 1_000_000;
const key = (t: number, v: number): Key => ({ tUs: t * S, v });

/** The fixture with its title (1 s to 4 s) moving right between 0.5 s and 2 s into it, and fading out at 2 s. */
function moving() {
  const snapshot = fixtureSnapshot();
  const item = snapshot.project.timeline.tracks[0].items[0];
  item.layout.motion = {
    ...item.layout.motion,
    keys: { x: [key(0.5, 0), key(2, 0.2)], y: [], scale: [], rotation: [], opacity: [key(2, 1)] },
  };
  useProject.setState({ snapshot, pending: 0, apply });
  return snapshot;
}

const layoutOf = (call: Parameters<typeof apply>) =>
  (call[1][0] as unknown as { patch: { layout: Layout } }).patch.layout;
const diamonds = () => [...document.querySelectorAll<HTMLButtonElement>('.tl-key')];

beforeEach(() => {
  vi.clearAllMocks();
  useUi.setState({ pxPerSecond: 100 });
  useSpeech.setState({ ranges: [] });
  useSelection.setState({ selectedId: null, selectedIds: [] });
  usePlayback.setState({ playheadUs: 0, playing: false });
});

describe('motion keys on the timeline', () => {
  it('show as a diamond at each time a property has a key; a click picks the layer and the key', () => {
    moving();
    renderWithProviders(<Timeline />);
    const shown = diamonds();
    expect(shown).toHaveLength(2);
    // 100 px a second: 0.5 s and 2 s into the block.
    expect(shown.map((d) => d.style.left)).toEqual(['50px', '200px']);
    fireEvent.pointerDown(shown[1], { button: 0, pointerId: 1, clientX: 300 });
    fireEvent.pointerUp(shown[1], { pointerId: 1, clientX: 300 });
    expect(useSelection.getState().selectedId).toBe('title');
    expect(usePlayback.getState().playheadUs).toBe(3 * S);
    expect(apply).not.toHaveBeenCalled();
  });

  it('a drag moves every key at that time within the layer; Option copies them', async () => {
    moving();
    renderWithProviders(<Timeline />);
    const [, later] = diamonds();
    fireEvent.pointerDown(later, { button: 0, pointerId: 1, clientX: 300 });
    fireEvent.pointerMove(later, { pointerId: 1, clientX: 350 });
    fireEvent.pointerUp(later, { pointerId: 1, clientX: 350 });
    await act(async () => undefined);
    expect(apply.mock.calls[0][0]).toBe('keys.move.undo');
    const moved = layoutOf(apply.mock.calls[0]).motion!.keys!;
    expect(moved.x).toEqual([key(0.5, 0), key(2.5, 0.2)]);
    expect(moved.opacity).toEqual([key(2.5, 1)]);
    // Past the layer's end: held at it.
    fireEvent.pointerDown(later, { button: 0, pointerId: 2, clientX: 300, altKey: true });
    fireEvent.pointerUp(later, { pointerId: 2, clientX: 900, altKey: true });
    await act(async () => undefined);
    expect(apply.mock.calls[1][0]).toBe('keys.copy.undo');
    expect(layoutOf(apply.mock.calls[1]).motion!.keys!.x).toEqual([key(0.5, 0), key(2, 0.2), key(3, 0.2)]);
  });

  it('Delete on a diamond takes its keys away, not the layer; Clear motion keys takes them all', async () => {
    moving();
    renderWithProviders(<Timeline />);
    const [first] = diamonds();
    fireEvent.keyDown(first, { key: 'Delete' });
    await act(async () => undefined);
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toBe('keys.delete.undo');
    expect(layoutOf(apply.mock.calls[0]).motion!.keys!.x).toEqual([key(2, 0.2)]);
    await act(async () => void (await clearMotionKeys('title')));
    expect(apply.mock.calls[1][0]).toBe('keys.clear.undo');
    // Its fades stay; the keys go.
    const cleared = layoutOf(apply.mock.calls[1]).motion!;
    expect(cleared.keys).toBeUndefined();
    expect(cleared.enter).toBeDefined();
  });
});

describe('volume keys on the timeline', () => {
  /** The fixture's song (0 to 30 s) as a sound effect at 50 % with `keys`, selected. */
  function sound(keys: Key[] = []) {
    const snapshot = fixtureSnapshot();
    const audio = snapshot.project.timeline.tracks[1];
    const [song] = audio.items;
    const content = song.content as AudioContent;
    audio.items = [
      {
        ...song,
        content: { ...content, volume: 0.5, duck: undefined, fadeInUs: 0, fadeOutUs: 0, volumeKeys: keys },
      },
    ];
    useProject.setState({ snapshot, pending: 0, apply });
    useSelection.setState({ selectedId: song.id, selectedIds: [song.id] });
    return song.id;
  }
  const contentOf = (call: Parameters<typeof apply>) =>
    (call[1][0] as unknown as { patch: { content: AudioContent } }).patch.content;

  it('a selected sound shows its line; a click on it adds a key there at its level', async () => {
    sound();
    renderWithProviders(<Timeline />);
    const grip = document.querySelector('.tl-volume-grip')!;
    const svg = grip.closest('svg')!;
    svg.getBoundingClientRect = () => ({ left: 0, top: 0, right: 3000, bottom: 40, width: 3000, height: 40, x: 0, y: 0, toJSON: () => ({}) });
    fireEvent.pointerDown(grip, { button: 0, pointerId: 1, clientX: 400, clientY: 20 });
    fireEvent.pointerUp(grip, { pointerId: 1, clientX: 400, clientY: 20 });
    await act(async () => undefined);
    expect(apply.mock.calls[0][0]).toBe('volumeKeys.add.undo');
    expect(contentOf(apply.mock.calls[0]).volumeKeys).toEqual([key(4, 0.5)]);
  });

  it('its keys are points: a drag moves one, Delete takes it away, a drag of the line moves them all', async () => {
    sound([key(2, 0.5), key(6, 1)]);
    renderWithProviders(<Timeline />);
    const points = [...document.querySelectorAll('.tl-volume-key')];
    expect(points).toHaveLength(2);
    fireEvent.pointerDown(points[0], { button: 0, pointerId: 1, clientX: 200, clientY: 20 });
    fireEvent.pointerUp(points[0], { pointerId: 1, clientX: 300, clientY: 20 });
    await act(async () => undefined);
    expect(apply.mock.calls[0][0]).toBe('volumeKeys.move.undo');
    expect(contentOf(apply.mock.calls[0]).volumeKeys).toEqual([key(3, 0.5), key(6, 1)]);
    fireEvent.keyDown(points[1], { key: 'Delete' });
    await act(async () => undefined);
    expect(apply.mock.calls[1][0]).toBe('volumeKeys.remove.undo');
    expect(contentOf(apply.mock.calls[1]).volumeKeys).toEqual([key(2, 0.5)]);
    // Up the lane: every key louder.
    const grip = document.querySelector('.tl-volume-grip')!;
    fireEvent.pointerDown(grip, { button: 0, pointerId: 2, clientX: 500, clientY: 30 });
    fireEvent.pointerUp(grip, { pointerId: 2, clientX: 500, clientY: 20 });
    await act(async () => undefined);
    expect(apply.mock.calls[2][0]).toBe('volume.undo');
    const louder = contentOf(apply.mock.calls[2]).volumeKeys!;
    expect(louder[0].v).toBeGreaterThan(0.5);
    expect(louder[1].v).toBeGreaterThan(1);
  });
});
