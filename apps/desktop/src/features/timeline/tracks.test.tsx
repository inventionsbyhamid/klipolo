import { act, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * Show all tracks: every track in a row of its own with its header (its name, which a double click renames; Hide or
 * Mute, Solo and Lock; its options: Rename, the toggles, Move up and down, Delete track), the buttons that add a track,
 * and the new-track zone a dragged layer can go to. The fixture has Overlay 1 (a title) and Audio 1 (a sound effect).
 */

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useSelection } = await import('../../state/selection');
const { useToasts } = await import('../../state/toasts');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { HEADER_PX, TRACK_HEADER_PX } = await import('./layout');
const { TRACK_HEADER_CHROME_PX, TRACK_STATE_PX, TRACK_TOGGLE_PX, TRACK_TOGGLES_GAP_PX } =
  await import('./TrackHeader');
const { TRACK_CONTROL_PX } = await import('./lanes/TrackMenu');
const { useCommands } = await import('../command-palette/useCommands');

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));
const header = (name: string) => screen.getByText(name).closest<HTMLElement>('[data-track-header]')!;
/** The rows top to bottom, by lane or track, but for the recorded sounds' lanes under Video. */
const rows = () =>
  [...document.querySelectorAll<HTMLElement>('[data-lane]')]
    .map((row) => row.dataset.lane)
    .filter((lane) => !lane?.startsWith('source:'));

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  usePlayback.setState({ playheadUs: 25_000_000, playing: false });
  useSelection.setState({ selectedId: null });
  useUi.setState({ timelineExpanded: true, pxPerSecond: 40, snapping: true });
  useToasts.setState({ toasts: [] });
  vi.mocked(bridge.apply)
    .mockReset()
    .mockImplementation(async () => {
      const s = useProject.getState().snapshot!;
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { clips: [], items: [] },
      };
    });
});

describe('show all tracks', () => {
  it("the corner button and each lane's options show every track; the corner button shows fewer again", async () => {
    useUi.setState({ timelineExpanded: false });
    renderWithProviders(<Timeline />);
    expect(rows()).toEqual(['layers', 'main', 'sounds', 'music']);
    await userEvent.click(screen.getByRole('button', { name: 'Show all tracks' }));
    expect(useUi.getState().timelineExpanded).toBe(true);
    // The picture tracks top first, Video, then the audio tracks, each named in its header.
    expect(rows()).toEqual(['t1', 'main', 't2']);
    expect(screen.getByText('Overlay 1')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Show fewer tracks' }));
    expect(rows()).toEqual(['layers', 'main', 'sounds', 'music']);
    await userEvent.click(screen.getByRole('button', { name: 'Overlays options' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Show all tracks' }));
    expect(useUi.getState().timelineExpanded).toBe(true);
  });

  it('the headers get a column wide enough for track names', () => {
    renderWithProviders(<Timeline />);
    const column = header('Overlay 1').parentElement!;
    expect(column.style.width).toBe(`${TRACK_HEADER_PX}px`);
    useUi.setState({ timelineExpanded: false });
  });

  it('renames a track inline', async () => {
    renderWithProviders(<Timeline />);
    await userEvent.dblClick(screen.getByText('Overlay 1'));
    const field = screen.getByRole('textbox', { name: 'Track name' });
    await userEvent.clear(field);
    await userEvent.type(field, 'Titles{Enter}');
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.track.rename',
      ops: [{ type: 'updateTrack', id: 't1', patch: { name: 'Titles' } }],
    });
  });

  it('Escape keeps the old name and an empty name changes nothing', async () => {
    renderWithProviders(<Timeline />);
    await userEvent.dblClick(screen.getByText('Overlay 1'));
    await userEvent.keyboard('{Escape}');
    expect(screen.getByText('Overlay 1')).toBeInTheDocument();
    // Emptied (spaces only) and confirmed: nothing changes, and focus is back on the track's options button.
    await userEvent.dblClick(screen.getByText('Overlay 1'));
    await userEvent.clear(screen.getByRole('textbox', { name: 'Track name' }));
    await userEvent.keyboard('   {Enter}');
    await frames();
    expect(screen.getByText('Overlay 1')).toBeInTheDocument();
    expect(document.activeElement).toHaveAttribute('data-track-menu', 't1');
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it("the options' Rename makes the name a field", async () => {
    renderWithProviders(<Timeline />);
    await userEvent.click(screen.getByRole('button', { name: 'Track options', description: 'Audio 1' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Rename' }));
    await frames();
    expect(screen.getByRole('textbox', { name: 'Track name' })).toHaveFocus();
  });

  it('adds and deletes tracks from the rows and the menu', async () => {
    renderWithProviders(<Timeline />);
    await userEvent.click(screen.getByRole('button', { name: 'Add overlay track' }));
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'timeline.track.add',
      ops: [{ type: 'addTrack', id: null, kind: 'overlay', index: null }],
    });
    await userEvent.click(screen.getByRole('button', { name: 'Add audio track' }));
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'timeline.track.add',
      ops: [{ type: 'addTrack', id: null, kind: 'audio', index: null }],
    });
    await userEvent.click(screen.getAllByRole('button', { name: 'Track options' })[0]);
    await userEvent.click(screen.getByRole('menuitem', { name: 'Delete track' }));
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'timeline.track.delete',
      ops: [{ type: 'removeTrack', id: 't1' }],
    });
  });

  it('dropping a layer on the new-track zone makes a track for it', async () => {
    renderWithProviders(<Timeline />);
    const title = screen.getByRole('button', { name: /Set up billing/ });
    fireEvent.pointerDown(title, { pointerId: 1, button: 0, clientX: 100, clientY: 100 });
    fireEvent.pointerMove(title, { pointerId: 1, clientX: 104, clientY: 100 });
    await frames();
    const zone = document.querySelector<HTMLElement>('[data-new-track="overlay"]')!;
    zone.getBoundingClientRect = () =>
      ({ top: 0, bottom: 24, left: 0, right: 2000, height: 24, width: 2000 }) as DOMRect;
    fireEvent.pointerMove(title, { pointerId: 1, clientX: 104, clientY: 10 });
    await frames();
    fireEvent.pointerUp(title, { pointerId: 1, clientX: 104, clientY: 10 });
    await frames();
    const { label, ops } = vi.mocked(bridge.apply).mock.calls.at(-1)![0];
    expect(label).toBe('timeline.move.layerNewTrack.undo');
    expect(ops[0]).toMatchObject({ type: 'addTrack', kind: 'overlay', index: 2 });
    expect(ops[1]).toMatchObject({ type: 'moveItem', id: 'title', trackId: (ops[0] as { id: string }).id });
    // The zone goes once the drag has ended.
    expect(document.querySelector('[data-new-track]')).toBeNull();
  });

  it("a layer's menu moves it to a new track", async () => {
    renderWithProviders(<Timeline />);
    fireEvent.contextMenu(screen.getByRole('button', { name: /Set up billing/ }));
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Move to a new track' }));
    await frames();
    const { label, ops } = vi.mocked(bridge.apply).mock.calls.at(-1)![0];
    expect(label).toBe('timeline.move.layerNewTrack.undo');
    expect(ops[0]).toMatchObject({ type: 'addTrack', kind: 'overlay', index: 2 });
  });

  it('moves a track up and down among its own rows; the ends are off', async () => {
    // Overlay 2 (t3) draws above Overlay 1 (t1): the last picture track is on top.
    const snapshot = fixtureSnapshot();
    const [overlay] = snapshot.project.timeline.tracks;
    snapshot.project.timeline.tracks.push({ ...overlay, id: 't3', name: 'Overlay 2', items: [] });
    useProject.setState({ snapshot });
    renderWithProviders(<Timeline />);
    expect(rows()).toEqual(['t3', 't1', 'main', 't2']);
    await userEvent.click(screen.getByRole('button', { name: 'Track options', description: 'Overlay 1' }));
    expect(screen.getByRole('menuitem', { name: 'Move down' })).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(screen.getByRole('menuitem', { name: 'Move up' }));
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'timeline.track.move',
      ops: [{ type: 'moveTrack', id: 't1', index: 2 }],
    });
  });

  it('deleting a track with layers in it says what goes, and is one undo step', async () => {
    renderWithProviders(<Timeline />);
    await userEvent.click(screen.getByRole('button', { name: 'Track options', description: 'Overlay 1' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Delete track' }));
    await frames();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(useToasts.getState().toasts.at(-1)?.text).toBe('Deleted Overlay 1 and its 1 layer. ⌘Z brings them back.');
  });

  it("a locked track's Delete is off and says why", async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.tracks[0].locked = true;
    useProject.setState({ snapshot });
    renderWithProviders(<Timeline />);
    await userEvent.click(screen.getByRole('button', { name: 'Track options', description: /^Overlay 1/ }));
    const remove = screen.getByRole('menuitem', { name: 'Delete track' });
    expect(remove).toHaveAttribute('aria-disabled', 'true');
    expect(remove).toHaveAccessibleDescription('Unlock the track to delete it.');
  });

  it('solo: a header toggle on overlay and audio tracks (not zoom), and a badge at rest', async () => {
    const snapshot = fixtureSnapshot();
    const [overlay] = snapshot.project.timeline.tracks;
    snapshot.project.timeline.tracks.push(
      { ...overlay, id: 'z', kind: 'zoom', name: 'Zoom 1', items: [] },
      { ...overlay, id: 't3', name: 'Overlay 2', items: [], solo: true, locked: true },
    );
    useProject.setState({ snapshot });
    renderWithProviders(<Timeline />);
    await userEvent.click(within(header('Audio 1')).getByRole('button', { name: 'Solo Audio 1' }));
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'timeline.track.solo',
      ops: [{ type: 'updateTrack', id: 't2', patch: { solo: true } }],
    });
    await frames();
    expect(document.querySelector('.timeline [role="status"]')).toHaveTextContent(
      'Audio 1 is soloed: only soloed tracks play',
    );
    // A zoom track has no solo.
    expect(within(header('Zoom 1')).queryByRole('button', { name: /Solo/ })).toBeNull();
    // A soloed track says so at rest, and its button turns it off.
    expect(within(header('Overlay 2')).getByRole('img', { name: 'Soloed and locked' })).toBeInTheDocument();
    expect(within(header('Overlay 2')).getByRole('button', { name: 'Unsolo Overlay 2' })).toBeInTheDocument();
  });

  it("names get the header's width: at rest only the options button and a 12 px badge", () => {
    const snapshot = fixtureSnapshot();
    const [overlay] = snapshot.project.timeline.tracks;
    snapshot.project.timeline.tracks.push(
      { ...overlay, id: 't3', name: 'Overlay 2', items: [], locked: true },
      { ...overlay, id: 't4', name: 'Overlay 3', items: [], hidden: true, locked: true },
    );
    useProject.setState({ snapshot });
    renderWithProviders(<Timeline />);
    /** The controls the header shows before it is pointed at or focused. */
    const atRest = (el: HTMLElement) =>
      [...el.querySelectorAll('button')].filter((b) => !b.closest('[data-at-rest="hidden"]'));
    for (const name of ['Overlay 1', 'Overlay 2', 'Overlay 3'])
      expect(atRest(header(name)).map((b) => b.getAttribute('aria-label'))).toEqual(['Track options']);
    expect(within(header('Overlay 1')).queryByRole('img')).toBeNull();
    expect(within(header('Overlay 2')).getByRole('img', { name: 'Locked' })).toBeInTheDocument();
    expect(within(header('Overlay 3')).getByRole('img', { name: 'Hidden and locked' })).toBeInTheDocument();
    // The name has the column less its padding, the options button and the badge.
    expect(TRACK_HEADER_PX - TRACK_HEADER_CHROME_PX - TRACK_CONTROL_PX - TRACK_STATE_PX).toBeGreaterThanOrEqual(80);
    // Pointed at or focused, the toggles show in the header's own row, before the options button, inside the column.
    for (const name of ['Overlay 1', 'Audio 1']) {
      const toggles = header(name).querySelector<HTMLElement>('[data-track-toggles]')!;
      const buttons = [...toggles.querySelectorAll('button')];
      expect(buttons).toHaveLength(3);
      for (const button of buttons) expect(button.style.width).toBe(`${TRACK_TOGGLE_PX}px`);
      expect(toggles.parentElement).toBe(header(name));
      expect(toggles.nextElementSibling).toHaveAttribute('aria-label', 'Track options');
    }
    const fixed = TRACK_HEADER_CHROME_PX + TRACK_TOGGLES_GAP_PX + 3 * TRACK_TOGGLE_PX + TRACK_CONTROL_PX;
    expect(TRACK_HEADER_PX - fixed).toBeGreaterThanOrEqual(30);
    expect(within(header('Overlay 3')).getByRole('button', { name: 'Show Overlay 3' })).toBeInTheDocument();
    expect(within(header('Overlay 3')).getByRole('button', { name: 'Unlock Overlay 3' })).toBeInTheDocument();
    // Fewer tracks: the gutter is the lanes' icons' again.
    expect(HEADER_PX).toBeLessThan(TRACK_HEADER_PX);
  });

  it('a header toggle keeps focus as it flips', async () => {
    vi.mocked(bridge.apply).mockImplementationOnce(async () => {
      const s = useProject.getState().snapshot!;
      const tracks = s.project.timeline.tracks.map(({ items, ...tr }) => ({
        ...tr,
        hidden: tr.id === 't1' ? true : tr.hidden,
        items: items.map((i) => i.id),
      }));
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { tracks, clips: [], items: [] },
      };
    });
    renderWithProviders(<Timeline />);
    act(() => screen.getByRole('button', { name: 'Hide Overlay 1' }).focus());
    await userEvent.keyboard('{Enter}');
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.track.hide',
      ops: [{ type: 'updateTrack', id: 't1', patch: { hidden: true } }],
    });
    expect(document.activeElement).toHaveAccessibleName('Show Overlay 1');
  });

  it("the toggles stay a keystroke away: Shift+Tab from the track's options button reaches each", async () => {
    renderWithProviders(<Timeline />);
    act(() => screen.getByRole('button', { name: 'Track options', description: 'Audio 1' }).focus());
    for (const name of ['Lock Audio 1', 'Solo Audio 1', 'Mute Audio 1']) {
      await userEvent.tab({ shift: true });
      expect(document.activeElement).toHaveAccessibleName(name);
    }
    await userEvent.keyboard('{Enter}');
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.track.mute',
      ops: [{ type: 'updateTrack', id: 't2', patch: { muted: true } }],
    });
  });

  it('the toggles wait at rest, and never show while the name is edited', async () => {
    renderWithProviders(<Timeline />);
    const toggles = () => header('Audio 1').querySelector<HTMLElement>('[data-track-toggles]')!;
    expect(toggles()).toHaveAttribute('data-at-rest', 'hidden');
    expect(toggles()).not.toHaveAttribute('data-renaming');
    await userEvent.dblClick(screen.getByText('Audio 1'));
    expect(screen.getByRole('textbox', { name: 'Track name' })).toHaveFocus();
    expect(document.querySelector('[data-track-toggles][data-renaming]')).not.toBeNull();
    await userEvent.keyboard('{Escape}');
    expect(toggles()).not.toHaveAttribute('data-renaming');
  });

  it("the keys walk the tracks' rows: Down from a layer goes to Video", async () => {
    renderWithProviders(<Timeline />);
    const title = screen.getByRole('button', { name: /Set up billing/ });
    act(() => title.focus());
    fireEvent.keyDown(title, { key: 'ArrowDown' });
    expect(document.activeElement).toHaveAttribute('data-block-id', 'a');
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    expect(document.activeElement).toHaveAttribute('data-block-id', 'song');
  });

  it("the empty timeline's note starts after the gutter, as wide as the headers make it", () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.main = [];
    snapshot.project.timeline.tracks = snapshot.project.timeline.tracks.map((track) => ({ ...track, items: [] }));
    useProject.setState({ snapshot });
    const { container } = renderWithProviders(<Timeline />);
    expect(container.querySelector<HTMLElement>('.tl-empty')!.style.left).toBe(`${TRACK_HEADER_PX}px`);
    act(() => useUi.setState({ timelineExpanded: false }));
    expect(container.querySelector<HTMLElement>('.tl-empty')!.style.left).toBe(`${HEADER_PX}px`);
  });

  it("a waiting proposal outlines what it removes in the track's own row, a sound as its lane draws it", async () => {
    const { useProposal, noRemovals } = await import('../../state/proposal');
    const removals = { ...noRemovals(), items: ['song', 'title'] };
    act(() => useProposal.getState().show({ id: 'p', baseRevision: 3, tags: [], removals }));
    renderWithProviders(<Timeline />);
    const ghost = (track: string) =>
      document.querySelector<HTMLElement>(`[data-track-id="${track}"] [data-ghost="layer"]`)!;
    // The title fills its 28 px row 4 px in; the sound effect its 34 px row 2 px in.
    expect([ghost('t1').style.top, ghost('t1').style.height]).toEqual(['4px', '20px']);
    expect([ghost('t2').style.top, ghost('t2').style.height]).toEqual(['2px', '30px']);
    act(() => useProposal.getState().show(null));
  });

  it('is a command: Show all tracks, then Show fewer tracks', () => {
    useUi.setState({ timelineExpanded: false });
    let commands: ReturnType<typeof useCommands> = [];
    function Probe() {
      commands = useCommands();
      return null;
    }
    renderWithProviders(<Probe />);
    const toggle = () => commands.find((c) => c.id === 'toggle-tracks')!;
    expect(toggle().label).toBe('Show all tracks');
    act(() => toggle().run());
    expect(useUi.getState().timelineExpanded).toBe(true);
    expect(toggle().label).toBe('Show fewer tracks');
  });
});
