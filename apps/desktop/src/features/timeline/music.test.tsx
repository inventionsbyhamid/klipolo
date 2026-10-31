import { act, fireEvent, screen } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ItemContent } from '../../bindings/ItemContent';
import type { Op } from '../../bindings/Op';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The Music lane's volume line: the music's level with its fades, dipping under speech where it ducks,
 * the "Quieter while people talk" tag, and drags that change the volume (or, with Option, how deep it ducks).
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
const { deleteBlock } = await import('./commands');
const { toggleSoundMute } = await import('./keys/layerActions');
const { useSpeech } = await import('./media/speech');
const { Timeline } = await import('./Timeline');
const { levelPoints, levelY, ATTACK_US, RELEASE_US, DuckTag } = await import('./blocks/VolumeLine');
const { shiftBlock, sizeBlock, clearShifts } = await import('./drag/registry');

type AudioContent = Extract<ItemContent, { type: 'audio' }>;
const S = 1_000_000;

/** The fixture's song as music at 30 % from 0 to 30 s: ducking 12 dB, fading in 0.5 s and out 2 s. */
function music(duck = -12) {
  const snapshot = fixtureSnapshot();
  const audio = snapshot.project.timeline.tracks[1];
  const [song] = audio.items;
  audio.items = [
    {
      ...song,
      content: { ...(song.content as AudioContent), volume: 0.3, duck, fadeInUs: 500_000, fadeOutUs: 2 * S },
    },
  ];
  return snapshot;
}

/**
 * The fixture's song laid as one bed of three pieces back to back, 0 to 30 s, as `add_music_ops` lays a short song
 * under a longer video: one song, one level.
 */
function bed() {
  const snapshot = music();
  const audio = snapshot.project.timeline.tracks[1];
  const [song] = audio.items;
  const content = song.content as AudioContent;
  audio.items = ['bed', 'bed-2', 'bed-3'].map((id, k) => ({
    ...song,
    id,
    startUs: k * 10 * S,
    durationUs: 10 * S,
    content: { ...content, fadeInUs: k === 0 ? content.fadeInUs : 0, fadeOutUs: k === 2 ? content.fadeOutUs : 0 },
  }));
  return snapshot;
}

beforeEach(() => {
  vi.clearAllMocks();
  useUi.setState({ pxPerSecond: 40 });
  useSpeech.setState({ ranges: [{ start: 5 * S, end: 8 * S }] });
});

describe('the music’s volume line', () => {
  it('follows the level: up from the fade-in, down under speech with the mix’s ramps, out at the end', () => {
    const content = { volume: 0.3, duck: -12, fadeInUs: 500_000, fadeOutUs: 2 * S, muted: false };
    const points = levelPoints(30 * S, 0, content, [{ start: 5 * S, end: 8 * S }]);
    const at = (us: number) => points.find(([t]) => t === us)?.[1];
    expect(at(0)).toBe(0);
    expect(at(500_000)).toBeCloseTo(0.3);
    // Easing down 150 ms before the words, low under them, back up 400 ms after.
    expect(at(5 * S - ATTACK_US)).toBeCloseTo(0.3);
    expect(at(5 * S)).toBeCloseTo(0.3 * 10 ** (-12 / 20));
    expect(at(8 * S)).toBeCloseTo(0.3 * 10 ** (-12 / 20));
    expect(at(8 * S + RELEASE_US)).toBeCloseTo(0.3);
    expect(at(30 * S)).toBe(0);
    // Ducking off (0 dB) has no dips.
    const flat = levelPoints(30 * S, 0, { ...content, duck: 0 }, [{ start: 5 * S, end: 8 * S }]);
    expect(flat.filter(([t]) => t > 500_000 && t < 28 * S).every(([, g]) => Math.abs(g - 0.3) < 1e-9)).toBe(true);
    // Louder is higher up the lane.
    expect(levelY(0.3)).toBeLessThan(levelY(0.075));
  });

  it('draws in the Music lane with the tag where it ducks, and none where it doesn’t or the piece is too narrow', () => {
    useProject.setState({ snapshot: music(), pending: 0 });
    const { unmount } = renderWithProviders(<Timeline />);
    const lane = screen.getByRole('group', { name: 'Music' });
    expect(lane.querySelector('[data-testid="volume-line"]')).not.toBeNull();
    expect(lane).toHaveTextContent('Quieter while people talk');
    unmount();
    useProject.setState({ snapshot: music(0), pending: 0 });
    const flat = renderWithProviders(<Timeline />);
    expect(screen.getByRole('group', { name: 'Music' })).not.toHaveTextContent('Quieter while people talk');
    flat.unmount();
    // Zoomed out so the 30 s piece is 150 px: too narrow for the tag, which would cover the line.
    useUi.setState({ pxPerSecond: 5 });
    useProject.setState({ snapshot: music(), pending: 0 });
    renderWithProviders(<Timeline />);
    const narrow = screen.getByRole('group', { name: 'Music' });
    expect(narrow.querySelector('[data-testid="volume-line"]')).not.toBeNull();
    expect(narrow).not.toHaveTextContent('Quieter while people talk');
  });

  it('fades alone are no dip: music that ducks with nobody speaking has no tag', () => {
    useUi.setState({ pxPerSecond: 40 });
    useSpeech.setState({ ranges: [] });
    useProject.setState({ snapshot: music(-12), pending: 0 });
    renderWithProviders(<Timeline />);
    expect(screen.getByRole('group', { name: 'Music' })).not.toHaveTextContent('Quieter while people talk');
  });

  it('the tag sits beside the block, never in its button, and keeps to the right of what the lanes show', () => {
    useProject.setState({ snapshot: music(), pending: 0 });
    renderWithProviders(<Timeline />);
    const tag = screen.getByText('Quieter while people talk');
    // Sticky inside a button doesn't stick: the tag is the block's sibling, over the piece.
    expect(tag.closest('button')).toBeNull();
    const block = screen.getByRole('group', { name: 'Music' }).querySelector<HTMLElement>('[data-look="music"]')!;
    const band = tag.parentElement!;
    expect(band).toHaveClass('tl-duck-band');
    expect(band.style.left).toBe(`${Number(block.dataset.left) + 6}px`);
    expect(band.style.width).toBe(`${Number(block.dataset.width) - 12}px`);
    const appCss = readFileSync(resolve(__dirname, '../../styles/app.css'), 'utf8');
    expect(appCss).toMatch(/\.tl-duck-tag \{[^}]*position: sticky;[^}]*right: \d+px;/s);
    expect(appCss).toMatch(/\.tl-duck-band \{[^}]*position: absolute;[^}]*justify-content: flex-end;/s);
  });

  it('the tag goes with its block while a gesture moves it, hides while its edges are dragged, and dims with it', () => {
    useProject.setState({ snapshot: music(), pending: 0 });
    renderWithProviders(<Timeline />);
    const band = screen.getByText('Quieter while people talk').parentElement!;
    const block = screen.getByRole('group', { name: 'Music' }).querySelector<HTMLElement>('[data-look="music"]')!;
    const id = block.dataset.blockId!;
    act(() => shiftBlock(id, 40));
    expect(band.style.transform).toBe(block.style.transform);
    expect(band.style.transform).not.toBe('');
    act(() => sizeBlock(id, Number(block.dataset.left), 100));
    expect(band.style.visibility).toBe('hidden');
    act(() => clearShifts());
    expect([band.style.transform, band.style.visibility]).toEqual(['', '']);
    // Dimmed as its block is.
    const { container } = renderWithProviders(
      <DuckTag
        id="other"
        startUs={0}
        durationUs={30 * S}
        content={{ duck: -12, muted: false }}
        left={0}
        width={600}
        top={0}
        dimmed
      />,
    );
    expect(container.querySelector('.tl-duck-band')).toHaveClass('is-dimmed');
  });

  it('muted music under speech has no tag: its line is flat', () => {
    const snapshot = music();
    const [song] = snapshot.project.timeline.tracks[1].items;
    song.content = { ...(song.content as AudioContent), muted: true };
    useProject.setState({ snapshot, pending: 0 });
    renderWithProviders(<Timeline />);
    expect(screen.getByRole('group', { name: 'Music' })).not.toHaveTextContent('Quieter while people talk');
  });

  it('a drag of the line changes the volume, an Option drag how deep it ducks: one undo step each', async () => {
    useProject.setState({ snapshot: music(), pending: 0, apply });
    renderWithProviders(<Timeline />);
    const grip = screen.getByRole('group', { name: 'Music' }).querySelector('.tl-volume-grip')!;
    // Up the lane: louder.
    fireEvent.pointerDown(grip, { button: 0, pointerId: 1, clientY: 100 });
    fireEvent.pointerMove(grip, { pointerId: 1, clientY: 90 });
    fireEvent.pointerUp(grip, { pointerId: 1, clientY: 90 });
    await act(async () => undefined);
    expect(apply).toHaveBeenCalledTimes(1);
    const [label, ops] = apply.mock.calls[0];
    expect(label).toBe('volume.undo');
    const louder = (ops[0] as unknown as { patch: { content: AudioContent } }).patch.content;
    expect(louder.volume).toBeGreaterThan(0.3);
    expect(louder.duck).toBe(-12);
    // Option and down the lane: ducks deeper.
    fireEvent.pointerDown(grip, { button: 0, pointerId: 2, clientY: 100, altKey: true });
    fireEvent.pointerUp(grip, { pointerId: 2, clientY: 110 });
    await act(async () => undefined);
    expect(apply).toHaveBeenCalledTimes(2);
    expect(apply.mock.calls[1][0]).toBe('music.duck.undo');
    const deeper = (apply.mock.calls[1][1][0] as unknown as { patch: { content: AudioContent } }).patch.content;
    expect(deeper.duck).toBeLessThan(-12);
    expect(deeper.volume).toBe(0.3);
  });

  it('a song laid in pieces is one: a drag on one piece sets them all, each keeping its fades', async () => {
    useProject.setState({ snapshot: bed(), pending: 0, apply });
    useSelection.setState({ selectedId: null });
    renderWithProviders(<Timeline />);
    const lane = screen.getByRole('group', { name: 'Music' });
    const grips = lane.querySelectorAll('.tl-volume-grip');
    expect(grips).toHaveLength(3);
    fireEvent.pointerDown(grips[1], { button: 0, pointerId: 1, clientY: 100 });
    fireEvent.pointerUp(grips[1], { pointerId: 1, clientY: 90 });
    await act(async () => undefined);
    expect(apply).toHaveBeenCalledTimes(1);
    const [label, ops] = apply.mock.calls[0];
    expect(label).toBe('volume.undo');
    const set = ops.map((op) => {
      const { id, patch } = op as unknown as { id: string; patch: { content: AudioContent } };
      return [id, patch.content.volume, patch.content.fadeInUs, patch.content.fadeOutUs];
    });
    const louder = set[0][1] as number;
    expect(louder).toBeGreaterThan(0.3);
    // Each piece keeps its own fades.
    expect(set).toEqual([
      ['bed', louder, 500_000, 0],
      ['bed-2', louder, 0, 0],
      ['bed-3', louder, 0, 2 * S],
    ]);
  });

  it('selecting one piece of a song highlights the rest of it', () => {
    useProject.setState({ snapshot: bed(), pending: 0, apply });
    useSelection.setState({ selectedId: 'bed-2' });
    renderWithProviders(<Timeline />);
    const lane = screen.getByRole('group', { name: 'Music' });
    const block = (id: string) => lane.querySelector<HTMLElement>(`[data-block-id="${id}"]`)!;
    expect(block('bed-2')).toHaveClass('is-selected');
    expect(block('bed')).toHaveClass('is-in-run');
    expect(block('bed-3')).toHaveClass('is-in-run');
    expect(block('bed-2')).not.toHaveClass('is-in-run');
  });

  it('Delete on one piece of a song takes every piece, in one undo step', async () => {
    useProject.setState({ snapshot: bed(), pending: 0, apply });
    deleteBlock('bed-2', 'undo.delete.layer');
    await vi.waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(apply.mock.calls[0][0]).toBe('undo.delete.layer');
    expect(apply.mock.calls[0][1]).toEqual([
      { type: 'removeItem', id: 'bed' },
      { type: 'removeItem', id: 'bed-2' },
      { type: 'removeItem', id: 'bed-3' },
    ]);
  });

  it('Mute sound on one piece of a song mutes every piece, and Unmute sound unmutes them, one undo step each', async () => {
    useProject.setState({ snapshot: bed(), pending: 0, apply });
    toggleSoundMute('bed-2');
    await vi.waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    const set = (call: number) =>
      apply.mock.calls[call][1].map((op) => {
        const { id, patch } = op as unknown as { id: string; patch: { content: AudioContent } };
        return [id, patch.content.muted, patch.content.volume, patch.content.fadeInUs, patch.content.fadeOutUs];
      });
    expect(apply.mock.calls[0][0]).toBe('volume.mute.undo');
    // Every piece muted in one change, each keeping its volume and fades for when it plays again.
    expect(set(0)).toEqual([
      ['bed', true, 0.3, 500_000, 0],
      ['bed-2', true, 0.3, 0, 0],
      ['bed-3', true, 0.3, 0, 2 * S],
    ]);
    // The muted song: Unmute sound from its last piece brings back all three, in one change.
    const muted = bed();
    for (const piece of muted.project.timeline.tracks[1].items)
      piece.content = { ...(piece.content as AudioContent), muted: true };
    useProject.setState({ snapshot: muted, pending: 0, apply });
    toggleSoundMute('bed-3');
    await vi.waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
    expect(apply.mock.calls[1][0]).toBe('volume.unmute.undo');
    expect(set(1).map(([id, on]) => [id, on])).toEqual([
      ['bed', false],
      ['bed-2', false],
      ['bed-3', false],
    ]);
  });

  it('a drag that is refused puts the line back where the project has it', async () => {
    apply.mockResolvedValueOnce(false);
    useProject.setState({ snapshot: music(), pending: 0, apply });
    renderWithProviders(<Timeline />);
    const lane = screen.getByRole('group', { name: 'Music' });
    const line = lane.querySelector('.tl-volume-line')!;
    const grip = lane.querySelector('.tl-volume-grip')!;
    const before = line.getAttribute('points');
    fireEvent.pointerDown(grip, { button: 0, pointerId: 1, clientY: 100 });
    fireEvent.pointerMove(grip, { pointerId: 1, clientY: 80 });
    expect(line.getAttribute('points')).not.toBe(before);
    fireEvent.pointerUp(grip, { pointerId: 1, clientY: 80 });
    await act(async () => undefined);
    expect(apply).toHaveBeenCalledTimes(1);
    expect(line.getAttribute('points')).toBe(before);
  });
});
