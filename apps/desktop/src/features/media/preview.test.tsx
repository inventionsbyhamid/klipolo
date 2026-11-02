import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Asset } from '../../bindings/Asset';
import type { PackItem } from '../../bindings/PackItem';
import type { SnapshotPatch } from '../../bindings/SnapshotPatch';
import { fixtureSnapshot, revisionPatch } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * Hearing a sound before adding it: a Sounds or Music row's art is a play button ("Play Whoosh", then "Stop Whoosh")
 * that plays the WAV Rust decodes for it through an `Audio` element. One plays at a time; starting one pauses the
 * timeline, and adding it, the timeline playing, another tab, the panel folding or another project stops it.
 */

const answer = (): Promise<SnapshotPatch> => Promise.resolve(revisionPatch(4));
const PACK: PackItem[] = [
  { id: 'whoosh', name: 'Whoosh', category: 'sound', durationUs: 900_000, width: 0, height: 0, tags: [] },
  { id: 'impact', name: 'Impact', category: 'sound', durationUs: 400_000, width: 0, height: 0, tags: [] },
  { id: 'soft-ambient', name: 'Soft ambient', category: 'music', durationUs: 45_000_000, width: 0, height: 0, tags: [] },
];
vi.mock('../../bridge/library', () => ({
  library: {
    marks: vi.fn(() => Promise.resolve({})),
    setFavorite: vi.fn(() => Promise.resolve({})),
    markUsed: vi.fn(() => Promise.resolve({})),
    addMusic: vi.fn(() => answer()),
    addSound: vi.fn(() => answer()),
    addImage: vi.fn(() => answer()),
    place: vi.fn(() => answer().then((patch) => ({ patch, unplaced: [] }))),
    pack: vi.fn(() => Promise.resolve(PACK)),
    picture: vi.fn(() => Promise.reject(new Error('no pictures in tests'))),
    sound: vi.fn(() => Promise.resolve(new ArrayBuffer(44))),
    speechRanges: vi.fn(() => Promise.resolve([])),
  },
}));
vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    play: vi.fn(() => Promise.resolve()),
    pause: vi.fn(() => Promise.resolve(2_000_000)),
    seek: vi.fn(() => Promise.resolve()),
  },
}));
vi.mock('../timeline/drag/mediaDrag', async (original) => ({
  ...(await original<typeof import('../timeline/drag/mediaDrag')>()),
  beginMediaDrag: vi.fn(),
}));
const { library } = await import('../../bridge/library');
const { engine } = await import('../../bridge/engine');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { useLibraryPanel } = await import('../../state/libraryPanel');
const { useCaptionsView } = await import('../../state/captionsView');
const { useToasts } = await import('../../state/toasts');
const { usePack } = await import('./shelves');
const { useMarks } = await import('./marks');
const { stopPreview, togglePreview, usePreview } = await import('./preview');
const { beginMediaDrag } = await import('../timeline/drag/mediaDrag');
const { Library } = await import('../../layout/Library');

/** A stand-in for the page's `Audio`: it plays at once, and the test says when it ends or fails. */
class FakeAudio {
  static made: FakeAudio[] = [];
  paused = true;
  private heard = new Map<string, (() => void)[]>();
  constructor(public src: string) {
    FakeAudio.made.push(this);
  }
  play = vi.fn(() => {
    this.paused = false;
    return Promise.resolve();
  });
  pause = vi.fn(() => {
    this.paused = true;
  });
  /** Its sound let go: no source, loaded again. */
  removeAttribute = vi.fn((name: string) => {
    if (name === 'src') this.src = '';
  });
  load = vi.fn();
  addEventListener(type: string, listener: () => void) {
    this.heard.set(type, [...(this.heard.get(type) ?? []), listener]);
  }
  fire(type: string) {
    for (const listener of this.heard.get(type) ?? []) listener();
  }
}
vi.stubGlobal('Audio', FakeAudio);

const kilnDoor: Asset = {
  id: 'kiln-door',
  kind: 'audio',
  path: '/media/kiln-door',
  name: 'kiln-door.wav',
  durationUs: 2_000_000,
  width: 0,
  height: 0,
  fps: 0,
  hasAudio: true,
};

beforeEach(() => {
  vi.clearAllMocks();
  stopPreview();
  FakeAudio.made = [];
  const snapshot = fixtureSnapshot();
  snapshot.project.assets.push(kilnDoor);
  // No project, then this one: the sounds an earlier test had are let go.
  useProject.setState({ snapshot: null });
  useProject.setState({ snapshot, pending: 0 });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false, rate: 1 });
  useUi.setState({ libraryTab: 'sounds' });
  useLibraryPanel.setState({ collapsed: false });
  useCaptionsView.setState({ view: 'library', open: false });
  usePack.setState({ items: [], asked: false });
  useMarks.setState({ marks: {}, asked: false });
  useToasts.setState({ toasts: [] });
});

/** The row named `name`, once the pack is in. */
const rowOf = async (name: string) => (await screen.findByText(name)).closest('li')!;
/** The Audio made last. */
const lastAudio = () => FakeAudio.made.at(-1)!;

describe('a sound’s play button', () => {
  it('plays the sound, shows it playing, and stops it when pressed again', async () => {
    renderWithProviders(<Library />);
    const whoosh = await rowOf('Whoosh');
    await userEvent.click(within(whoosh).getByRole('button', { name: 'Play Whoosh' }));
    expect(library.sound).toHaveBeenCalledWith({ packId: 'whoosh' });
    await waitFor(() => expect(FakeAudio.made).toHaveLength(1));
    expect(lastAudio().src).toMatch(/^blob:/);
    expect(lastAudio().play).toHaveBeenCalledTimes(1);
    const stop = within(whoosh).getByRole('button', { name: 'Stop Whoosh' });
    expect(whoosh).toHaveClass('is-previewing');
    // Once it is heard, a bar along the row's foot runs over its length.
    await waitFor(() => expect(whoosh.querySelector('.preview-progress')).not.toBeNull());
    expect(whoosh.querySelector<HTMLElement>('.preview-progress')!.style.animationDuration).toBe('900ms');
    await userEvent.click(stop);
    expect(lastAudio().paused).toBe(true);
    // Its sound is let go at once.
    expect(lastAudio().src).toBe('');
    expect(lastAudio().load).toHaveBeenCalledTimes(1);
    expect(within(whoosh).getByRole('button', { name: 'Play Whoosh' })).toBeInTheDocument();
    expect(whoosh).not.toHaveClass('is-previewing');
    expect(whoosh.querySelector('.preview-progress')).toBeNull();
    // A second play has the sound already: Rust isn't asked again.
    await userEvent.click(within(whoosh).getByRole('button', { name: 'Play Whoosh' }));
    await waitFor(() => expect(FakeAudio.made).toHaveLength(2));
    expect(library.sound).toHaveBeenCalledTimes(1);
  });

  it('plays one sound at a time: another one stops the first', async () => {
    renderWithProviders(<Library />);
    const whoosh = await rowOf('Whoosh');
    const impact = await rowOf('Impact');
    await userEvent.click(within(whoosh).getByRole('button', { name: 'Play Whoosh' }));
    await waitFor(() => expect(FakeAudio.made).toHaveLength(1));
    const first = lastAudio();
    await userEvent.click(within(impact).getByRole('button', { name: 'Play Impact' }));
    await waitFor(() => expect(FakeAudio.made).toHaveLength(2));
    expect(first.paused).toBe(true);
    expect(lastAudio().paused).toBe(false);
    expect(library.sound).toHaveBeenLastCalledWith({ packId: 'impact' });
    expect(screen.getAllByRole('button', { name: /^Stop / })).toHaveLength(1);
    expect(within(whoosh).getByRole('button', { name: 'Play Whoosh' })).toBeInTheDocument();
    // The first one's end, after the second started, leaves the second playing.
    act(() => first.fire('ended'));
    expect(usePreview.getState().key).toBe('pack:impact');
    act(() => lastAudio().fire('ended'));
    expect(usePreview.getState().key).toBeNull();
    expect(within(impact).getByRole('button', { name: 'Play Impact' })).toBeInTheDocument();
  });

  it('a press while the sound is still coming plays only the newest', async () => {
    let arrive: (bytes: ArrayBuffer) => void = () => undefined;
    vi.mocked(library.sound).mockImplementationOnce(() => new Promise((resolve) => (arrive = resolve)));
    renderWithProviders(<Library />);
    await userEvent.click(within(await rowOf('Whoosh')).getByRole('button', { name: 'Play Whoosh' }));
    await userEvent.click(within(await rowOf('Impact')).getByRole('button', { name: 'Play Impact' }));
    await waitFor(() => expect(FakeAudio.made).toHaveLength(1));
    await act(async () => arrive(new ArrayBuffer(44)));
    expect(FakeAudio.made).toHaveLength(1);
    expect(usePreview.getState().key).toBe('pack:impact');
  });

  it('stops as the sound is added, or dragged onto the timeline', async () => {
    renderWithProviders(<Library />);
    const whoosh = await rowOf('Whoosh');
    await userEvent.click(within(whoosh).getByRole('button', { name: 'Play Whoosh' }));
    await waitFor(() => expect(FakeAudio.made).toHaveLength(1));
    await userEvent.click(within(whoosh).getByRole('button', { name: 'Add Whoosh at the playhead' }));
    expect(lastAudio().paused).toBe(true);
    expect(within(whoosh).getByRole('button', { name: 'Play Whoosh' })).toBeInTheDocument();
    await waitFor(() => expect(library.addSound).toHaveBeenCalledWith({ type: 'pack', id: 'whoosh' }, 2_000_000));
    // Dragged: it stops as it lands.
    await userEvent.click(within(whoosh).getByRole('button', { name: 'Play Whoosh' }));
    await waitFor(() => expect(FakeAudio.made).toHaveLength(2));
    fireEvent.pointerDown(whoosh, { button: 0, pointerId: 1 });
    const drop = vi.mocked(beginMediaDrag).mock.calls.at(-1)![2];
    act(() => drop({ lane: 'sound', trackId: null, atUs: 4_000_000, row: 'sounds' }));
    expect(lastAudio().paused).toBe(true);
    expect(usePreview.getState().key).toBeNull();
  });

  it('pauses the timeline, and stops when the timeline plays', async () => {
    usePlayback.setState({ playing: true });
    renderWithProviders(<Library />);
    const whoosh = await rowOf('Whoosh');
    await userEvent.click(within(whoosh).getByRole('button', { name: 'Play Whoosh' }));
    expect(usePlayback.getState().playing).toBe(false);
    expect(engine.pause).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(FakeAudio.made).toHaveLength(1));
    expect(lastAudio().paused).toBe(false);
    act(() => usePlayback.getState().play());
    expect(lastAudio().paused).toBe(true);
    expect(within(whoosh).getByRole('button', { name: 'Play Whoosh' })).toBeInTheDocument();
  });

  it('stops when the tab changes, the panel folds, another view shows or another project opens', async () => {
    renderWithProviders(<Library />);
    const play = async (name: string) => {
      await userEvent.click(within(await rowOf(name)).getByRole('button', { name: `Play ${name}` }));
      await waitFor(() => expect(lastAudio().paused).toBe(false));
      return lastAudio();
    };
    let playing = await play('Whoosh');
    await userEvent.click(screen.getByRole('tab', { name: 'Music' }));
    expect(playing.paused).toBe(true);
    // A music bed plays from its start, its first 30 s at most.
    playing = await play('Soft ambient');
    expect(library.sound).toHaveBeenLastCalledWith({ packId: 'soft-ambient' });
    await waitFor(() =>
      expect(
        (screen.getByText('Soft ambient').closest('li')!.querySelector('.preview-progress') as HTMLElement).style
          .animationDuration,
      ).toBe('30000ms'),
    );
    act(() => useLibraryPanel.getState().setCollapsed(true));
    expect(playing.paused).toBe(true);
    act(() => useLibraryPanel.getState().setCollapsed(false));
    playing = await play('Soft ambient');
    act(() => useCaptionsView.getState().setView('captions'));
    expect(playing.paused).toBe(true);
    act(() => useCaptionsView.getState().setView('library'));
    playing = await play('Soft ambient');
    act(() => useProject.setState({ snapshot: { ...fixtureSnapshot(), opening: 2 } }));
    expect(playing.paused).toBe(true);
    expect(usePreview.getState().key).toBeNull();
  });

  it('plays your own sounds by their file', async () => {
    renderWithProviders(<Library />);
    const row = (await screen.findByText('kiln-door.wav')).closest('li')!;
    await userEvent.click(within(row).getByRole('button', { name: 'Play kiln-door.wav' }));
    expect(library.sound).toHaveBeenCalledWith({ path: '/media/kiln-door' });
    await waitFor(() => expect(lastAudio().paused).toBe(false));
    expect(usePreview.getState().key).toBe('file:/media/kiln-door');
  });

  it('a sound that can’t be had or played says so and shows play again', async () => {
    vi.mocked(library.sound).mockRejectedValueOnce(new Error('That sound couldn’t be played.'));
    renderWithProviders(<Library />);
    const whoosh = await rowOf('Whoosh');
    await userEvent.click(within(whoosh).getByRole('button', { name: 'Play Whoosh' }));
    await waitFor(() => expect(useToasts.getState().toasts.map((t) => t.text)).toEqual(['That sound couldn’t be played.']));
    expect(within(whoosh).getByRole('button', { name: 'Play Whoosh' })).toBeInTheDocument();
    expect(FakeAudio.made).toHaveLength(0);
    // The page can't play what came: it says so too.
    await userEvent.click(within(whoosh).getByRole('button', { name: 'Play Whoosh' }));
    await waitFor(() => expect(FakeAudio.made).toHaveLength(1));
    act(() => lastAudio().fire('error'));
    expect(within(whoosh).getByRole('button', { name: 'Play Whoosh' })).toBeInTheDocument();
    expect(useToasts.getState().toasts).toHaveLength(2);
  });

  it('leaves Space to the timeline: Space on the play button plays the sound, on the row the timeline', async () => {
    const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');
    function Editor() {
      useEditorShortcuts();
      return <Library />;
    }
    renderWithProviders(<Editor />);
    const whoosh = await rowOf('Whoosh');
    within(whoosh).getByRole('button', { name: 'Play Whoosh' }).focus();
    await userEvent.keyboard(' ');
    await waitFor(() => expect(FakeAudio.made.at(-1)?.paused).toBe(false));
    expect(usePreview.getState().key).toBe('pack:whoosh');
    expect(engine.play).not.toHaveBeenCalled();
    // Pressed again, the same button stops it.
    expect(within(whoosh).getByRole('button', { name: 'Stop Whoosh' })).toHaveFocus();
    await userEvent.keyboard(' ');
    expect(usePreview.getState().key).toBeNull();
    expect(engine.play).not.toHaveBeenCalled();
    // Space on the row itself plays the timeline.
    fireEvent.keyDown(whoosh, { key: ' ' });
    expect(engine.play).toHaveBeenCalledTimes(1);
    expect(usePlayback.getState().playing).toBe(true);
    expect(library.sound).toHaveBeenCalledTimes(1);
  });

  it('the art drags the row as the rest of it does; its click plays, and a drag that ends on it doesn’t', async () => {
    renderWithProviders(<Library />);
    const whoosh = await rowOf('Whoosh');
    const art = within(whoosh).getByRole('button', { name: 'Play Whoosh' });
    fireEvent.pointerDown(art, { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
    expect(beginMediaDrag).toHaveBeenCalledTimes(1);
    expect(vi.mocked(beginMediaDrag).mock.calls[0][1]).toMatchObject({ id: 'klipolo-pack-whoosh', name: 'Whoosh' });
    // Let go far from where it was pressed: a drag, so it doesn't play.
    fireEvent.click(art, { detail: 1, clientX: 140, clientY: 300 });
    expect(usePreview.getState().key).toBeNull();
    // A click where it was pressed plays.
    fireEvent.pointerDown(art, { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
    fireEvent.click(art, { detail: 1, clientX: 11, clientY: 10 });
    expect(usePreview.getState().key).toBe('pack:whoosh');
    await waitFor(() => expect(FakeAudio.made).toHaveLength(1));
  });

  it('stops when the search changes, as its row may go', async () => {
    renderWithProviders(<Library />);
    const whoosh = await rowOf('Whoosh');
    await userEvent.click(within(whoosh).getByRole('button', { name: 'Play Whoosh' }));
    await waitFor(() => expect(FakeAudio.made).toHaveLength(1));
    await userEvent.type(screen.getByRole('searchbox', { name: 'Search sounds' }), 'imp');
    expect(lastAudio().paused).toBe(true);
    expect(usePreview.getState().key).toBeNull();
    expect(screen.queryByText('Whoosh')).toBeNull();
  });

  it('keeps the last 8 sounds, lets the oldest go, and lets them all go as another project opens', async () => {
    const made = vi.spyOn(URL, 'createObjectURL');
    const revoked = vi.spyOn(URL, 'revokeObjectURL');
    const play = (n: number) => act(() => togglePreview(`pack:s${n}`, { packId: `s${n}` }));
    for (let n = 0; n < 8; n++) await play(n);
    expect(library.sound).toHaveBeenCalledTimes(8);
    expect(revoked).not.toHaveBeenCalled();
    const urls = made.mock.results.map((r) => r.value as string);
    // A ninth lets the oldest go.
    await play(8);
    await waitFor(() => expect(revoked.mock.calls).toEqual([[urls[0]]]));
    // One kept plays without asking again; the one let go is asked for again (and the next oldest goes).
    await play(5);
    expect(library.sound).toHaveBeenCalledTimes(9);
    await play(0);
    expect(library.sound).toHaveBeenCalledTimes(10);
    expect(library.sound).toHaveBeenLastCalledWith({ packId: 's0' });
    await waitFor(() => expect(revoked.mock.calls).toEqual([[urls[0]], [urls[1]]]));
    // Another project: every kept sound goes.
    act(() => useProject.setState({ snapshot: { ...fixtureSnapshot(), opening: 3 } }));
    await waitFor(() => expect(revoked).toHaveBeenCalledTimes(10));
    expect(new Set(revoked.mock.calls.map(([url]) => url)).size).toBe(10);
    await play(5);
    expect(library.sound).toHaveBeenCalledTimes(11);
    made.mockRestore();
    revoked.mockRestore();
  });
});
