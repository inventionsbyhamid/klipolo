import { act, fireEvent } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
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
vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn(), redo: vi.fn() },
  mediaUrl: (p: string) => p,
}));
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
const { deletePickedKey, useKeyPick } = await import('../../state/keyPick');
const { bridge } = await import('../../bridge/commands');

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

/** An apply that lands as Rust's does for the layer patches here: the changed layer is in the project afterwards. */
function landing(_label: string, ops: Op[]): Promise<boolean> {
  const snapshot = structuredClone(useProject.getState().snapshot!);
  for (const op of ops) {
    if (op.type !== 'updateItem') continue;
    for (const track of snapshot.project.timeline.tracks)
      track.items = track.items.map((item) => (item.id === op.id ? ({ ...item, ...op.patch } as Item) : item));
  }
  useProject.setState({ snapshot });
  return Promise.resolve(true);
}

/** An undo as Rust answers it: `item` (the layer as it was) back in the project, at the next revision. */
function undoTo(item: Item) {
  const { history } = useProject.getState().snapshot!;
  return Promise.resolve({
    baseRevision: history.revision,
    history: { ...history, revision: history.revision + 1 },
    patch: { clips: [], items: [item] },
  });
}

/** A drag of `el` from `fromX` to `toX` (and down by `dy`) that lets go there. */
function drag(el: Element, fromX: number, toX: number, dy = 0) {
  fireEvent.pointerDown(el, { button: 0, pointerId: 9, clientX: fromX, clientY: 20 });
  fireEvent.pointerMove(el, { pointerId: 9, clientX: toX, clientY: 20 + dy });
  fireEvent.pointerUp(el, { pointerId: 9, clientX: toX, clientY: 20 + dy });
}

beforeEach(() => {
  vi.clearAllMocks();
  useUi.setState({ pxPerSecond: 100 });
  useSpeech.setState({ ranges: [] });
  useSelection.setState({ selectedId: null, selectedIds: [] });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useKeyPick.getState().clear();
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
    fireEvent.pointerDown(first, { button: 0, pointerId: 1, clientX: 150 });
    fireEvent.pointerUp(first, { pointerId: 1, clientX: 150 });
    fireEvent.keyDown(first, { key: 'Delete' });
    await act(async () => undefined);
    expect(apply).toHaveBeenCalledTimes(1);
    const { useKeyPick } = await import('../../state/keyPick');
    expect(useKeyPick.getState().picked).toBeNull();
    expect(apply.mock.calls[0][0]).toBe('keys.delete.undo');
    expect(layoutOf(apply.mock.calls[0]).motion!.keys!.x).toEqual([key(2, 0.2)]);
    await act(async () => void (await clearMotionKeys('title')));
    expect(apply.mock.calls[1][0]).toBe('keys.clear.undo');
    // Its fades stay; the keys go.
    const cleared = layoutOf(apply.mock.calls[1]).motion!;
    expect(cleared.keys).toBeUndefined();
    expect(cleared.enter).toBeDefined();
  });

  it('Delete straight after a drag takes the moved keys, not the layer', async () => {
    moving();
    const lands = vi.fn(landing);
    useProject.setState({ apply: lands });
    renderWithProviders(<Timeline />);
    const [, later] = diamonds();
    // Picked first, then dragged from 2 s to 2.5 s into the layer.
    fireEvent.pointerDown(later, { button: 0, pointerId: 1, clientX: 300 });
    fireEvent.pointerUp(later, { pointerId: 1, clientX: 300 });
    drag(later, 300, 350);
    await act(async () => undefined);
    expect(lands.mock.calls[0][0]).toBe('keys.move.undo');
    expect(useKeyPick.getState().picked).toEqual({ kind: 'motion', itemId: 'title', tUs: 2.5 * S });
    expect(deletePickedKey()).toBe(true);
    await act(async () => undefined);
    expect(lands.mock.calls[1][0]).toBe('keys.delete.undo');
    const left = layoutOf(lands.mock.calls[1]).motion!.keys!;
    expect(left.x).toEqual([key(0.5, 0)]);
    expect(left.opacity).toEqual([]);
    expect(lands.mock.calls.flatMap(([, ops]) => ops.map((op) => op.type))).not.toContain('removeItem');
  });

  it('Delete straight after an Option drag takes the copy, not the keys it was copied from', async () => {
    moving();
    const lands = vi.fn(landing);
    useProject.setState({ apply: lands });
    renderWithProviders(<Timeline />);
    const [first] = diamonds();
    // Copied from 0.5 s to 1 s into the layer.
    fireEvent.pointerDown(first, { button: 0, pointerId: 1, clientX: 150, altKey: true });
    fireEvent.pointerMove(first, { pointerId: 1, clientX: 200, altKey: true });
    fireEvent.pointerUp(first, { pointerId: 1, clientX: 200, altKey: true });
    await act(async () => undefined);
    expect(lands.mock.calls[0][0]).toBe('keys.copy.undo');
    expect(useKeyPick.getState().picked).toEqual({ kind: 'motion', itemId: 'title', tUs: 1 * S });
    // The pressed diamond stays, but lets go of the focus: Delete goes to the picked copy, not to it.
    expect(diamonds()).toContain(first);
    expect(document.activeElement).not.toBe(first);
    fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Delete' });
    expect(deletePickedKey()).toBe(true);
    await act(async () => undefined);
    expect(lands).toHaveBeenCalledTimes(2);
    expect(lands.mock.calls[1][0]).toBe('keys.delete.undo');
    expect(layoutOf(lands.mock.calls[1]).motion!.keys!.x).toEqual([key(0.5, 0), key(2, 0.2)]);
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

  it('its keys are points beside the block, each a button named by its time and level', () => {
    const id = sound([key(2, 0.5), key(6, 1)]);
    renderWithProviders(<Timeline />);
    const points = [...document.querySelectorAll<HTMLElement>('.tl-volume-key')];
    expect(points).toHaveLength(2);
    // Not inside the block's own button: each can take the focus, and a screen reader hears it.
    const block = document.querySelector(`[data-block-id="${id}"]`)!;
    expect(points.every((p) => p.tagName === 'BUTTON' && !block.contains(p))).toBe(true);
    expect(points[0]).toHaveAccessibleName('Volume key at 0:02.0, 50%');
    points[0].focus();
    expect(document.activeElement).toBe(points[0]);
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
    // A key gone by its own Delete is let go: the next Delete acts on the selection again.
    const { useKeyPick } = await import('../../state/keyPick');
    expect(useKeyPick.getState().picked).toBeNull();
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

  it('Delete straight after a drag takes the moved point, not the sound', async () => {
    const id = sound([key(2, 0.5), key(6, 1)]);
    const lands = vi.fn(landing);
    useProject.setState({ apply: lands });
    renderWithProviders(<Timeline />);
    const [first] = document.querySelectorAll('.tl-volume-key');
    // From 2 s to 3 s into the sound, at its level.
    drag(first, 200, 300);
    await act(async () => undefined);
    expect(lands.mock.calls[0][0]).toBe('volumeKeys.move.undo');
    expect(contentOf(lands.mock.calls[0]).volumeKeys).toEqual([key(3, 0.5), key(6, 1)]);
    expect(useKeyPick.getState().picked).toEqual({ kind: 'volume', target: { kind: 'sound', id }, tUs: 3 * S });
    expect(deletePickedKey()).toBe(true);
    await act(async () => undefined);
    expect(lands.mock.calls[1][0]).toBe('volumeKeys.remove.undo');
    expect(contentOf(lands.mock.calls[1]).volumeKeys).toEqual([key(6, 1)]);
    expect(lands.mock.calls.flatMap(([, ops]) => ops.map((op) => op.type))).not.toContain('removeItem');
  });

  it('a dragged point lets go of the focus, so a Delete before the move lands takes it where it went', async () => {
    sound([key(2, 0.5), key(6, 1)]);
    let land!: () => void;
    const slow = vi.fn((label: string, ops: Op[]) =>
      label === 'volumeKeys.move.undo'
        ? new Promise<boolean>((done) => (land = () => void landing(label, ops).then(done)))
        : landing(label, ops),
    );
    useProject.setState({ apply: slow });
    renderWithProviders(<Timeline />);
    const [first] = document.querySelectorAll<HTMLElement>('.tl-volume-key');
    drag(first, 200, 300);
    await act(async () => undefined);
    expect(slow).toHaveBeenCalledTimes(1);
    // Still drawn at 2 s until the move is in, but not focused: its own Delete would take nothing there by then.
    expect(document.activeElement).not.toBe(first);
    fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Delete' });
    expect(deletePickedKey()).toBe(true);
    await act(async () => land());
    await vi.waitFor(() => expect(slow).toHaveBeenCalledTimes(2));
    expect(slow.mock.calls[1][0]).toBe('volumeKeys.remove.undo');
    expect(contentOf(slow.mock.calls[1]).volumeKeys).toEqual([key(6, 1)]);
  });

  it('an undo after a drag lets the point go, so Delete acts on the selection again', async () => {
    sound([key(2, 0.5), key(6, 1)]);
    const [song] = useProject.getState().snapshot!.project.timeline.tracks[1].items;
    const lands = vi.fn(landing);
    useProject.setState({ apply: lands });
    vi.mocked(bridge.undo).mockImplementation(() => undoTo(song));
    renderWithProviders(<Timeline />);
    drag(document.querySelector('.tl-volume-key')!, 200, 300);
    await act(async () => undefined);
    expect(useKeyPick.getState().picked?.tUs).toBe(3 * S);
    // ⌘Z: the point goes back to 2 s, and nothing is picked or drawn picked.
    await act(async () => useProject.getState().undo());
    expect(bridge.undo).toHaveBeenCalledTimes(1);
    const back = useProject.getState().snapshot!.project.timeline.tracks[1].items[0].content as AudioContent;
    expect(back.volumeKeys).toEqual([key(2, 0.5), key(6, 1)]);
    expect(useKeyPick.getState().picked).toBeNull();
    expect(document.querySelector('.tl-volume-key[data-picked]')).toBeNull();
    expect(deletePickedKey()).toBe(false);
  });
});

describe('a picked key', () => {
  it('Delete takes the picked key away, and after a press elsewhere the selection again', async () => {
    moving();
    renderWithProviders(<Timeline />);
    const [first] = diamonds();
    fireEvent.pointerDown(first, { button: 0, pointerId: 1, clientX: 150 });
    fireEvent.pointerUp(first, { pointerId: 1, clientX: 150 });
    expect(useKeyPick.getState().picked).toEqual({ kind: 'motion', itemId: 'title', tUs: 0.5 * S });
    expect(first).toHaveAttribute('data-picked');
    await act(async () => void deletePickedKey());
    expect(apply.mock.calls[0][0]).toBe('keys.delete.undo');
    expect(useKeyPick.getState().picked).toBeNull();
    // A press on anything else lets a picked key go.
    fireEvent.pointerDown(first, { button: 0, pointerId: 2, clientX: 150 });
    fireEvent.pointerUp(first, { pointerId: 2, clientX: 150 });
    fireEvent.pointerDown(document.body, { button: 0, pointerId: 3 });
    expect(useKeyPick.getState().picked).toBeNull();
    expect(deletePickedKey()).toBe(false);
    // A pick of a key that is gone is let go and changes nothing: Delete acts on the selection, as with none picked.
    const before = apply.mock.calls.length;
    useKeyPick.getState().pick({ kind: 'motion', itemId: 'title', tUs: 7 * S });
    expect(deletePickedKey()).toBe(false);
    await act(async () => undefined);
    expect(apply.mock.calls.slice(before).flatMap(([, ops]) => ops)).toEqual([]);
    expect(useKeyPick.getState().picked).toBeNull();
    // Another selection lets a pick go.
    useKeyPick.getState().pick({ kind: 'motion', itemId: 'title', tUs: 2 * S });
    act(() => useSelection.getState().select('b'));
    expect(useKeyPick.getState().picked).toBeNull();
  });

  it('a Delete pressed before a drag has landed takes the key once it is there', async () => {
    moving();
    let land!: () => void;
    const slow = vi.fn((label: string, ops: Op[]) =>
      label === 'keys.move.undo'
        ? new Promise<boolean>((done) => (land = () => void landing(label, ops).then(done)))
        : landing(label, ops),
    );
    useProject.setState({ apply: slow });
    renderWithProviders(<Timeline />);
    const [first] = diamonds();
    // From 0.5 s to 1 s into the layer; Delete before the move is in.
    drag(first, 150, 200);
    await act(async () => undefined);
    expect(slow).toHaveBeenCalledTimes(1);
    expect(deletePickedKey()).toBe(true);
    await act(async () => land());
    await vi.waitFor(() => expect(slow).toHaveBeenCalledTimes(2));
    expect(slow.mock.calls[1][0]).toBe('keys.delete.undo');
    expect(layoutOf(slow.mock.calls[1]).motion!.keys!.x).toEqual([key(2, 0.2)]);
  });

  it('a picked key taken away by another edit (chat, a voice) lets Delete act on the selection', async () => {
    const snapshot = moving();
    renderWithProviders(<Timeline />);
    const [first] = diamonds();
    fireEvent.pointerDown(first, { button: 0, pointerId: 1, clientX: 150 });
    fireEvent.pointerUp(first, { pointerId: 1, clientX: 150 });
    expect(useKeyPick.getState().picked?.tUs).toBe(0.5 * S);
    // Chat clears the layer's motion keys.
    const next = structuredClone(snapshot);
    const title = next.project.timeline.tracks[0].items[0];
    delete title.layout.motion!.keys;
    act(() => useProject.setState({ snapshot: next }));
    expect(deletePickedKey()).toBe(false);
    await act(async () => undefined);
    expect(apply).not.toHaveBeenCalled();
  });
});
