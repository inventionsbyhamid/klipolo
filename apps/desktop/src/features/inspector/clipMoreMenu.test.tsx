import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ClipFraming } from '../../bindings/ClipFraming';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The clip card's more menu lists the clip's own actions as its block's right-click menu does, from the same commands:
 * Freeze frame (⌥F), Reverse for an imported video, and Separate camera, Separate sound or a recorded sound's Separate,
 * each off with the same reason where it can't act. Then Duplicate and Delete.
 */

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn() },
  mediaUrl: (p: string) => p,
}));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const fill = { mode: 'fill', focusX: 0.5, focusY: 0.5 } as const;
const WIDE = { x: 0, y: 0, w: 1, h: 1 };
const FRAMING: ClipFraming = {
  framing: { landscape: fill, portrait: fill, square: fill },
  follows: false,
  speaker: false,
  fillCrop: { landscape: WIDE, portrait: WIDE, square: WIDE },
};
vi.mock('../../bridge/framing', () => ({ clipFraming: vi.fn(() => Promise.resolve(FRAMING)) }));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useSelection } = await import('../../state/selection');
const { Inspector } = await import('./Inspector');

const S = 1_000_000;
const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));

/** The card's more menu, open; its items by name (an item's name starts with its label, then its shortcut). */
async function openMore() {
  await userEvent.click(screen.getByRole('button', { name: 'More' }));
  return (name: string) => screen.queryByRole('menuitem', { name: new RegExp(`^${name}`) });
}
const isOff = (item: HTMLElement | null) => item?.getAttribute('aria-disabled') === 'true';

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: 'b' });
  usePlayback.setState({ playheadUs: 25 * S, playing: false });
  vi.mocked(bridge.apply).mockReset().mockResolvedValue({ ...fixturePatch(fixtureSnapshot()), baseRevision: 3 });
});

describe("the clip card's more menu", () => {
  it('lists Freeze frame with its shortcut, Reverse and Separate sound for an imported video, then Duplicate and Delete', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    const item = await openMore();
    expect(item('Freeze frame')).toHaveTextContent('⌥F');
    expect(isOff(item('Freeze frame'))).toBe(false);
    expect(isOff(item('Reverse'))).toBe(false);
    expect(isOff(item('Separate sound'))).toBe(false);
    expect(item('Duplicate')).toBeInTheDocument();
    expect(item('Delete')).toBeInTheDocument();
    const names = screen.getAllByRole('menuitem').map((m) => m.textContent);
    expect(names.indexOf('Duplicate')).toBeGreaterThan(names.findIndex((n) => n?.startsWith('Separate sound')));
  });

  it('Freeze frame holds this clip’s frame at the playhead, as its block menu does', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    const item = await openMore();
    await userEvent.click(item('Freeze frame')!);
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.freeze',
      ops: [{ type: 'freezeFrame', atUs: 25 * S, durationUs: 2 * S }],
    });
  });

  it('Separate sound puts the clip’s sound on a layer of its own', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    const item = await openMore();
    await userEvent.click(item('Separate sound')!);
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.sound.separate',
      ops: [{ type: 'detachSound', clipId: 'b' }],
    });
  });

  it('an item that cannot act is off and says why, as in the block menu', async () => {
    const fast = fixtureSnapshot();
    fast.project.timeline.main[1].speed = 2;
    useProject.setState({ snapshot: fast });
    usePlayback.setState({ playheadUs: 90 * S });
    renderWithProviders(<Inspector format="landscape" />);
    const item = await openMore();
    expect(isOff(item('Separate sound'))).toBe(true);
    expect(item('Separate sound')).toHaveAccessibleDescription(
      'Set the clip back to normal speed before separating its sound.',
    );
    expect(isOff(item('Freeze frame'))).toBe(true);
    expect(item('Freeze frame')).toHaveAccessibleDescription("There's no clip at the playhead to freeze.");
  });

  it('a recording clip lists Freeze frame, Separate camera and its sounds’ Separate, with no Reverse or Separate sound', async () => {
    useSelection.setState({ selectedId: 'a' });
    usePlayback.setState({ playheadUs: 5 * S });
    renderWithProviders(<Inspector format="landscape" />);
    const item = await openMore();
    expect(item('Freeze frame')).toBeInTheDocument();
    expect(item('Separate camera')).toBeInTheDocument();
    expect(item('Separate Mic')).toBeInTheDocument();
    expect(item('Reverse')).toBeNull();
    expect(item('Separate sound')).toBeNull();
  });
});
