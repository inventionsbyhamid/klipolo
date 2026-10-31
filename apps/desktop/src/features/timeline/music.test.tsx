import { act, fireEvent, screen } from '@testing-library/react';
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
const { useSpeech } = await import('./media/speech');
const { Timeline } = await import('./Timeline');
const { levelPoints, levelY, ATTACK_US, RELEASE_US } = await import('./blocks/VolumeLine');

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

  it('draws in the Music lane with the tag where it ducks, and none where it doesn’t', () => {
    useProject.setState({ snapshot: music(), pending: 0 });
    const { unmount } = renderWithProviders(<Timeline />);
    const lane = screen.getByRole('group', { name: 'Music' });
    expect(lane.querySelector('[data-testid="volume-line"]')).not.toBeNull();
    expect(lane).toHaveTextContent('Quieter while people talk');
    unmount();
    useProject.setState({ snapshot: music(0), pending: 0 });
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
});
