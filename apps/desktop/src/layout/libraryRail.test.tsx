import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ThumbStrip } from '../bindings/ThumbStrip';
import { fixtureSnapshot, revisionPatch } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/*
 * The library folds to its rail, as chat folds to its strip, and opens again from it. Folded, its rail shows the
 * clips' thumbnails, each doing what its row does: a press adds the clip to the end, a drag puts it on the timeline.
 */

vi.mock('../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../bridge/library', () => ({
  library: {
    pack: vi.fn(() => Promise.resolve([])),
    picture: vi.fn(() => Promise.reject(new Error('no pictures in tests'))),
    speechRanges: vi.fn(() => Promise.resolve([])),
    marks: vi.fn(() => Promise.resolve({})),
    markUsed: vi.fn((key: string) => Promise.resolve({ [key]: { favorite: false, usedMs: Date.now() } })),
  },
}));
/** The screen recording's thumbnails are scanned, 160 x 90 tiles 10 to a row; broll's aren't yet. */
const SCREEN_STRIP: ThumbStrip = {
  tileW: 160,
  tileH: 90,
  cols: 10,
  perChunk: 100,
  slotUs: 1_000_000,
  chunks: [{ index: 0, path: '/thumbs/screen-0.jpg', full: false }],
};
vi.mock('../bridge/media', () => ({
  media: { thumbs: vi.fn((path: string) => Promise.resolve(path === '/media/screen' ? SCREEN_STRIP : null)) },
}));
// A thumbnail's drag hands its drop to the test.
vi.mock('../features/timeline/drag/mediaDrag', async (original) => ({
  ...(await original<typeof import('../features/timeline/drag/mediaDrag')>()),
  beginMediaDrag: vi.fn(),
}));
const { bridge } = await import('../bridge/commands');
const { useProject } = await import('../state/project');
const { useLibraryPanel } = await import('../state/libraryPanel');
const { useMarks } = await import('../features/media/marks');
const { beginMediaDrag } = await import('../features/timeline/drag/mediaDrag');
const { Library } = await import('./Library');

beforeEach(() => {
  vi.mocked(bridge.apply).mockReset();
  vi.mocked(beginMediaDrag).mockClear();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useLibraryPanel.setState({ collapsed: false });
  useMarks.setState({ marks: {}, asked: false });
});

describe('the library folded', () => {
  it('is a rail with Open library; opening it shows the library again', async () => {
    renderWithProviders(<Library />);
    const panel = screen.getByRole('complementary', { name: 'Library' });
    act(() => useLibraryPanel.getState().setCollapsed(true));
    expect(panel).toHaveAttribute('data-collapsed', 'true');
    expect(screen.queryByRole('tab', { name: 'Clips' })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Open library' }));
    expect(useLibraryPanel.getState().collapsed).toBe(false);
    expect(panel).toHaveAttribute('data-collapsed', 'false');
    expect(screen.getByRole('tab', { name: 'Clips' })).toBeInTheDocument();
    // The keyboard goes on to the view's tab.
    expect(screen.getByRole('tab', { name: 'Library' })).toHaveFocus();
  });

  it('folded with the keyboard inside, it keeps the keyboard on its rail', () => {
    renderWithProviders(<Library />);
    act(() => screen.getByRole('tab', { name: 'Clips' }).focus());
    act(() => useLibraryPanel.getState().setCollapsed(true));
    expect(screen.getByRole('button', { name: 'Open library' })).toHaveFocus();
  });

  it('Collapse library in its head folds it, as chat has Collapse chat, and the rail keeps the keyboard', async () => {
    renderWithProviders(<Library />);
    const collapse = screen.getByRole('button', { name: 'Collapse library' });
    expect(collapse).toHaveAttribute('title', 'Collapse library (Command B)');
    expect(screen.queryByRole('button', { name: 'Open library' })).toBeNull();
    await userEvent.click(collapse);
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    expect(screen.getByRole('button', { name: 'Open library' })).toHaveFocus();
  });

  it("a double click on its edge folds it, and on the rail's edge opens it again", () => {
    const { container } = renderWithProviders(<Library />);
    const edge = () => container.querySelector<HTMLElement>('[data-library-edge]')!;
    fireEvent.doubleClick(edge());
    expect(useLibraryPanel.getState().collapsed).toBe(true);
    fireEvent.doubleClick(edge());
    expect(useLibraryPanel.getState().collapsed).toBe(false);
  });
});

describe('the folded library’s thumbnails', () => {
  /** The rail's list of clips. */
  const railClips = () => screen.getByRole('list', { name: 'Your clips' });

  it('the rail shows each clip’s thumbnail under Open library and Add files, named by the clip', async () => {
    renderWithProviders(<Library />);
    act(() => useLibraryPanel.getState().setCollapsed(true));
    const clips = within(railClips()).getAllByRole('button');
    // As the Clips tab lists them: the recording as one, then broll.
    expect(clips.map((b) => b.getAttribute('aria-label'))).toEqual(['Recording 1', 'broll.mp4']);
    expect(clips.map((b) => b.getAttribute('title'))).toEqual(['Recording 1', 'broll.mp4']);
    const rail = railClips().closest('.library-rail')!;
    const buttons = within(rail as HTMLElement).getAllByRole('button');
    expect(buttons.slice(0, 2).map((b) => b.getAttribute('aria-label'))).toEqual(['Open library', 'Add files']);
    // The recording's picture is its screen's first thumbnail, filling the square; broll keeps the clip art for now.
    await waitFor(() => expect(clips[0].querySelector('.rail-tile')).not.toBeNull());
    const tile = clips[0].querySelector<HTMLElement>('.rail-tile')!;
    expect(tile.style.backgroundImage).toBe('url("/thumbs/screen-0.jpg")');
    // The sheet is 10 tiles wide (its height follows).
    expect(tile.style.backgroundSize).toMatch(/^1000%/);
    // A wide tile is as tall as the square and centred, its sides cut off.
    expect(parseFloat(tile.style.width)).toBeCloseTo((160 / 90) * 100, 3);
    expect(tile.style.height).toBe('100%');
    expect(clips[1].querySelector('.rail-tile')).toBeNull();
    expect(clips[1].querySelector('svg')).not.toBeNull();
  });

  it('a press adds the clip to the end as its row’s "+" does; Enter too', async () => {
    vi.mocked(bridge.apply).mockResolvedValue(revisionPatch(4));
    renderWithProviders(<Library />);
    act(() => useLibraryPanel.getState().setCollapsed(true));
    await userEvent.click(within(railClips()).getByRole('button', { name: 'broll.mp4' }));
    await waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(vi.mocked(bridge.apply).mock.calls[0][0].ops).toMatchObject([
      { type: 'insertClip', index: 2, clip: { source: { type: 'asset', id: 'broll' } } },
    ]);
    act(() => within(railClips()).getByRole('button', { name: 'Recording 1' }).focus());
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(2));
    expect(vi.mocked(bridge.apply).mock.calls[1][0].ops).toMatchObject([
      { type: 'insertClip', clip: { source: { type: 'recording', id: 'rec' } } },
    ]);
  });

  it('a drag puts the clip on the timeline as its row’s does, and adds nothing on letting go', async () => {
    vi.mocked(bridge.apply).mockResolvedValue(revisionPatch(4));
    renderWithProviders(<Library />);
    act(() => useLibraryPanel.getState().setCollapsed(true));
    const take = within(railClips()).getByRole('button', { name: 'Recording 1' });
    fireEvent.pointerDown(take, { button: 0, pointerId: 1, clientX: 20, clientY: 200 });
    const [, carried, onDrop, lanes] = vi.mocked(beginMediaDrag).mock.calls.at(-1)!;
    expect(carried).toMatchObject({ name: 'Recording 1', durationUs: 60_000_000 });
    // A whole recording lands on the Video lane only, as its row's drag.
    expect(lanes).toBe('main');
    // The press moved past the slop: its click adds nothing.
    fireEvent.click(take, { clientX: 300, clientY: 400, detail: 1 });
    act(() => onDrop({ lane: 'main', index: 0 }));
    await waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(vi.mocked(bridge.apply).mock.calls[0][0].ops).toMatchObject([
      { type: 'insertClip', index: 0, clip: { source: { type: 'recording', id: 'rec' } } },
    ]);
  });
});
