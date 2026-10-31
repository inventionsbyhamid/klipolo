import { renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Asset } from '../../bindings/Asset';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { longProject } from '../../test/longProject';

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn(), importMedia: vi.fn() },
  mediaUrl: (p: string) => p,
}));
vi.mock('../../bridge/library', () => ({
  library: { addMusic: vi.fn(), place: vi.fn(), speechRanges: vi.fn(() => Promise.resolve([])) },
}));
// The real snap edges, counted: a drag builds them once per revision, not per frame.
vi.mock('../timeline/model/snap', async (importOriginal) => {
  const real = await importOriginal<typeof import('../timeline/model/snap')>();
  return { ...real, snapTargets: vi.fn(real.snapTargets) };
});
const { snapTargets } = await import('../timeline/model/snap');
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useSelection } = await import('../../state/selection');
const { useToasts } = await import('../../state/toasts');
const { importFiles } = await import('./useImportMedia');
const { useFinderDrop } = await import('./useFinderDrop');
const { useUi } = await import('../../state/ui');
const { registerDropTarget, timelineTargetAt, timelineLaneAt, dropAutoscroll, dropRefusal } = await import(
  '../timeline/dropTarget'
);
const { library } = await import('../../bridge/library');

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 2_000_000 });
  useToasts.setState({ toasts: [] });
  vi.mocked(bridge.apply).mockReset().mockResolvedValue(fixturePatch(fixtureSnapshot()));
  vi.mocked(bridge.importMedia).mockReset();
  vi.mocked(bridge.undo).mockReset();
  vi.mocked(library.addMusic).mockReset().mockResolvedValue(fixturePatch(fixtureSnapshot()));
  vi.mocked(library.place)
    .mockReset()
    .mockResolvedValue({ patch: fixturePatch(fixtureSnapshot()), unplaced: [] });
});

describe('importing media', () => {
  it('files dropped on the window import as one undo step, videos after the last clip in the order given', async () => {
    const broll = fixtureSnapshot().project.assets[5];
    const a: Asset = { ...broll, id: 'n1', path: '/n1.mp4', name: 'n1.mp4' };
    const b: Asset = { ...broll, id: 'n2', path: '/n2.mp4', name: 'n2.mp4' };
    vi.mocked(bridge.importMedia).mockResolvedValue({ assets: [a, b], skipped: [] });
    await importFiles(['/n1.mp4', '/n2.mp4']);
    expect(bridge.importMedia).toHaveBeenCalledWith(['/n1.mp4', '/n2.mp4']);
    await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    const tx = vi.mocked(bridge.apply).mock.calls[0][0];
    expect(tx.label).toBe('media.import');
    expect(tx.ops).toMatchObject([
      { type: 'addAssets', assets: [{ id: 'n1' }, { id: 'n2' }] },
      // `index: null` appends: after the last clip, in the order given.
      { type: 'insertClip', index: null, clip: { source: { type: 'asset', id: 'n1' } } },
      { type: 'insertClip', index: null, clip: { source: { type: 'asset', id: 'n2' } } },
    ]);
  });

  it('says what came in, with an Undo that takes back the import and nothing made since', async () => {
    const broll = fixtureSnapshot().project.assets[5];
    vi.mocked(bridge.importMedia).mockResolvedValue({
      assets: [
        { ...broll, id: 'n1', path: '/n1.mp4', name: 'n1.mp4' },
        { ...broll, id: 'n2', path: '/n2.mp4', name: 'n2.mp4' },
      ],
      skipped: [],
    });
    const imported = fixtureSnapshot();
    imported.history = { ...imported.history, revision: 4, undoLabel: 'media.import' };
    vi.mocked(bridge.apply).mockResolvedValue({ ...fixturePatch(imported), baseRevision: 3 });
    await importFiles(['/n1.mp4', '/n2.mp4']);
    await vi.waitFor(() => expect(useToasts.getState().toasts).toHaveLength(1));
    const [added] = useToasts.getState().toasts;
    expect([added.kind, added.text, added.done, added.actions?.map((a) => a.label)]).toEqual([
      'info',
      'Added 2 clips',
      true,
      ['Undo'],
    ]);
    vi.mocked(bridge.undo).mockResolvedValue({ ...fixturePatch(fixtureSnapshot()), baseRevision: 4 });
    added.actions![0].run();
    await vi.waitFor(() => expect(bridge.undo).toHaveBeenCalledTimes(1));
    // Once something else is on top, its Undo takes nothing back.
    useProject.setState({
      snapshot: { ...imported, history: { ...imported.history, revision: 5, undoLabel: 'text.add' } },
    });
    added.actions![0].run();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(bridge.undo).toHaveBeenCalledTimes(1);
  });

  it('names the files it skipped with why: a note when something came in, an error when nothing did', async () => {
    const broll = fixtureSnapshot().project.assets[5];
    vi.mocked(bridge.importMedia).mockResolvedValueOnce({
      assets: [{ ...broll, id: 'n1', path: '/n1.mp4', name: 'n1.mp4' }],
      skipped: [
        { name: 'notes.txt', reason: "This file type isn't supported." },
        { name: 'broken.mov', reason: "This file couldn't be read." },
      ],
    });
    await importFiles(['/n1.mp4', '/notes.txt', '/broken.mov']);
    expect(useToasts.getState().toasts.map((t) => [t.kind, t.text])).toEqual([
      [
        'info',
        "Skipped 2 files. notes.txt: this file type isn't supported. broken.mov: this file couldn't be read.",
      ],
    ]);
    useToasts.setState({ toasts: [] });
    vi.mocked(bridge.importMedia).mockResolvedValueOnce({
      assets: [],
      skipped: [{ name: 'notes.txt', reason: "This file type isn't supported." }],
    });
    await importFiles(['/notes.txt']);
    expect(useToasts.getState().toasts.map((t) => [t.kind, t.text])).toEqual([
      ['error', "Skipped notes.txt: this file type isn't supported."],
    ]);
  });

  it('names skipped files as they are, a # in a name included', async () => {
    const unsupported = "This file type isn't supported.";
    vi.mocked(bridge.importMedia).mockResolvedValueOnce({
      assets: [],
      skipped: ['Take #3.mov', '#1.txt', 'b.txt', 'c.txt', 'd.txt'].map((name) => ({ name, reason: unsupported })),
    });
    await importFiles(['/x']);
    expect(useToasts.getState().toasts.map((t) => t.text)).toEqual([
      "Skipped 5 files. Take #3.mov, #1.txt, b.txt and 2 more: this file type isn't supported.",
    ]);
  });

  it('nothing is placed into a project opened while the files were read', async () => {
    let read!: (value: { assets: Asset[]; skipped: [] }) => void;
    vi.mocked(bridge.importMedia).mockReturnValue(new Promise((resolve) => (read = resolve)));
    const importing = importFiles(['/n1.mp4']);
    useProject.setState({
      snapshot: { ...fixtureSnapshot(), project: { ...fixtureSnapshot().project, id: 'another' } },
    });
    read({ assets: [{ ...fixtureSnapshot().project.assets[5], id: 'n1' }], skipped: [] });
    await importing;
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it('the Finder listener mounts without Tauri', () => {
    const { unmount } = renderHook(() => useFinderDrop());
    unmount();
  });
});

describe('where a drop lands on the timeline', () => {
  afterEach(() => registerDropTarget(null));

  it('files dropped on the timeline import and land where they were dropped, as one undo step', async () => {
    const clip: Asset = { ...fixtureSnapshot().project.assets[5], id: 'n', path: '/n.mp4', name: 'n.mp4' };
    vi.mocked(bridge.importMedia).mockResolvedValue({ assets: [clip], skipped: [] });
    await importFiles(['/n.mp4'], { lane: 'main', index: 1 });
    expect(bridge.importMedia).toHaveBeenCalledWith(['/n.mp4']);
    await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    const tx = vi.mocked(bridge.apply).mock.calls[0][0];
    expect(tx.label).toBe('media.import');
    expect(tx.ops).toMatchObject([{ type: 'addAssets' }, { type: 'insertClip', index: 1 }]);
  });

  /** A lane row from `top`, `height` px tall (jsdom has no layout). */
  function row(attrs: Record<string, string>, top: number, height: number): HTMLElement {
    const el = document.createElement('div');
    for (const [name, value] of Object.entries(attrs)) el.setAttribute(name, value);
    el.getBoundingClientRect = () => DOMRect.fromRect({ x: 0, y: top, width: 1240, height });
    return el;
  }
  /** Klipolo's four lanes at 10 px/s under a 20 px ruler: Overlays, Video, Sounds and Music. */
  function lanes() {
    const scroller = row({}, 0, 300);
    scroller.append(
      row({ 'data-lane': 'layers' }, 20, 24),
      row({ 'data-lane': 'main' }, 49, 64),
      row({ 'data-lane': 'sounds' }, 118, 22),
      row({ 'data-lane': 'music' }, 145, 34),
    );
    return scroller;
  }
  // The 40 px gutter, then 10 px a second.
  const x = (seconds: number) => 40 + seconds * 10;

  it('maps the pointer to the cut or the lane under it', () => {
    useUi.setState({ pxPerSecond: 10, snapping: false });
    const at = (s: number, y: number) => timelineTargetAt(lanes(), 20, x(s), y);
    // Cuts at 0, 20 and 30 s.
    expect(at(24, 80)).toEqual({ lane: 'main', index: 1 });
    expect(at(26, 80)).toEqual({ lane: 'main', index: 2 });
    expect(at(24.01, 30)).toEqual({ lane: 'picture', trackId: null, atUs: 24_000_000 });
    // Sound effects and music are both sound layers.
    expect(at(3, 125)).toEqual({ lane: 'sound', trackId: null, atUs: 3_000_000, row: 'sounds' });
    // Over the Music lane a sound is music: added there as music that fits the video.
    expect(at(3, 160)).toEqual({ lane: 'sound', trackId: null, atUs: 3_000_000, row: 'music' });
    // The ruler and below the lanes take nothing.
    expect(at(3, 10)).toBeNull();
    expect(at(3, 250)).toBeNull();
    // Snapping on, a layer's start holds to a cut within 8 px (0.8 s here).
    useUi.setState({ snapping: true });
    expect(at(20.5, 125)).toMatchObject({ atUs: 20_000_000 });
  });

  it('only a lane that takes what is dragged is a target, so the indicator shows where it really lands', () => {
    useUi.setState({ pxPerSecond: 10, snapping: false });
    const at = (s: number, y: number, kind: Asset['kind'] | null) =>
      timelineTargetAt(lanes(), 20, x(s), y, kind);
    // A sound over the Overlays lane, a video or an image over the Sounds lane: nothing.
    expect(at(3, 30, 'audio')).toBeNull();
    expect(at(3, 125, 'video')).toBeNull();
    expect(at(3, 160, 'image')).toBeNull();
    expect(at(3, 125, 'audio')).toMatchObject({ lane: 'sound' });
    // The main track takes videos, and files from Finder of mixed kinds; a sound or a picture goes on its own lane.
    expect(at(24, 80, 'video')).toEqual({ lane: 'main', index: 1 });
    expect(at(24, 80, null)).toEqual({ lane: 'main', index: 1 });
    expect(at(24, 80, 'audio')).toBeNull();
    expect(at(24, 80, 'image')).toBeNull();
    expect(at(3, 160, null)).toMatchObject({ lane: 'sound' });
  });

  it('let go over a lane that won’t take it, media says which lane does', () => {
    useUi.setState({ pxPerSecond: 10, snapping: false });
    const scroller = lanes();
    registerDropTarget(
      (cx, cy, kind) => timelineTargetAt(scroller, 20, cx, cy, kind),
      scroller,
      (cx, cy) => timelineLaneAt(scroller, 20, cx, cy),
    );
    expect(dropRefusal(x(3), 80, 'image')).toBe('Pictures go on the Overlays lane.');
    expect(dropRefusal(x(3), 125, 'image')).toBe('Pictures go on the Overlays lane.');
    expect(dropRefusal(x(3), 30, 'audio')).toBe('Sounds go on the Sounds or Music lane.');
    expect(dropRefusal(x(3), 80, 'audio')).toBe('Sounds go on the Sounds or Music lane.');
    expect(dropRefusal(x(3), 160, 'video')).toBe('Videos go on the Video or Overlays lane.');
    // Where it lands, off the lanes, or a kind not known: nothing to say.
    expect(dropRefusal(x(3), 30, 'image')).toBeNull();
    expect(dropRefusal(x(3), 250, 'image')).toBeNull();
    expect(dropRefusal(x(3), 80, null)).toBeNull();
  });

  it('Finder files name their kind by extension: one kind, or none when mixed', async () => {
    const { kindOfPaths } = await import('../timeline/dropTarget');
    expect(kindOfPaths(['/a.PNG', '/b.heic'])).toBe('image');
    expect(kindOfPaths(['/song.mp3'])).toBe('audio');
    expect(kindOfPaths(['/clip.mov', '/loop.gif'])).toBe('video');
    expect(kindOfPaths(['/clip.mov', '/song.mp3'])).toBeNull();
    expect(kindOfPaths(['/notes.txt'])).toBeNull();
  });

  it('a picture dropped on the Overlays lane while its tracks are all locked goes on a new track, never a locked one', async () => {
    const snapshot = fixtureSnapshot();
    for (const track of snapshot.project.timeline.tracks) track.locked = true;
    useProject.setState({ snapshot, pending: 0 });
    useUi.setState({ pxPerSecond: 10, snapping: false });
    // Over the Overlays lane at 3 s: the lane takes a picture, and names no track.
    const target = timelineTargetAt(lanes(), 20, x(3), 30, 'image');
    expect(target).toEqual({ lane: 'picture', trackId: null, atUs: 3_000_000 });
    const logo: Asset = { ...snapshot.project.assets.find((a) => a.kind === 'image')!, id: 'n', path: '/n.png' };
    vi.mocked(bridge.importMedia).mockResolvedValue({ assets: [logo], skipped: [] });
    await importFiles(['/n.png'], target);
    // Placed by Rust as the "+" places a picture, the file coming in with it, on no named track: never the locked one.
    await vi.waitFor(() => expect(library.place).toHaveBeenCalledTimes(1));
    expect(library.place).toHaveBeenCalledWith([{ type: 'file', asset: logo }], 3_000_000, { type: 'auto' });
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it('several files let go on the Music lane: the first sound is music from there, the rest go to the library', async () => {
    useUi.setState({ pxPerSecond: 10, snapping: false });
    const { assets } = fixtureSnapshot().project;
    const song: Asset = { ...assets.find((a) => a.kind === 'audio')!, id: 'lofi', path: '/lofi.mp3', name: 'lofi.mp3' };
    const pop: Asset = { ...song, id: 'pop', path: '/pop.wav', name: 'pop.wav', durationUs: 400_000 };
    const sticker: Asset = { ...assets.find((a) => a.kind === 'image')!, id: 'st', path: '/st.png', name: 'st.png' };
    vi.mocked(bridge.importMedia).mockResolvedValue({ assets: [song, pop, sticker], skipped: [] });
    await importFiles(['/lofi.mp3', '/pop.wav', '/st.png'], {
      lane: 'sound',
      trackId: null,
      atUs: 6_000_000,
      row: 'music',
    });
    await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(library.addMusic).toHaveBeenCalledWith({ type: 'file', asset: song }, 6_000_000);
    // In the same turn, after the music: the others where Add files puts them (the library).
    const tx = vi.mocked(bridge.apply).mock.calls[0][0];
    expect(tx.ops).toEqual([{ type: 'addAssets', assets: [pop, sticker] }]);
    expect(vi.mocked(library.addMusic).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(bridge.apply).mock.invocationCallOrder[0],
    );
    await vi.waitFor(() =>
      expect(useToasts.getState().toasts.map((toast) => toast.text)).toContain(
        'Added lofi.mp3 as music, 2 files to the library',
      ),
    );
  });

  it('a Finder drop for another project, or one the close gave up on, places nothing, music included', async () => {
    const { assets } = fixtureSnapshot().project;
    const song: Asset = { ...assets.find((a) => a.kind === 'audio')!, id: 'lofi', path: '/lofi.mp3', name: 'lofi.mp3' };
    let answer!: (value: { assets: Asset[]; skipped: [] }) => void;
    vi.mocked(bridge.importMedia).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    const importing = importFiles(['/lofi.mp3'], { lane: 'sound', trackId: null, atUs: 0, row: 'music' });
    // Another project opened while the files were read.
    const other = fixtureSnapshot();
    other.project.id = 'other';
    useProject.setState({ snapshot: other });
    answer({ assets: [song], skipped: [] });
    await importing;
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(library.addMusic).not.toHaveBeenCalled();
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it('follows the playhead as it plays, with the hour’s edges built once per revision', () => {
    // The 1-hour project (1,500 cuts, 300 titles) at 10 px/s, scrolled to 10:00 and dragged over the Overlays lane for
    // 600 frames while it plays: each frame the pointer is on the playhead's frame, and the drop holds to it.
    const snap = fixtureSnapshot();
    useProject.setState({ snapshot: { ...snap, project: longProject() } });
    useUi.setState({ pxPerSecond: 10, snapping: true });
    vi.mocked(snapTargets).mockClear();
    const scroller = row({}, 0, 300);
    scroller.append(row({ 'data-lane': 'layers' }, 20, 60));
    scroller.scrollLeft = 5_900;
    const frame = (f: number) => Math.round((f * 1e6) / 30);
    for (let i = 0; i < 600; i++) {
      const playheadUs = frame(18_000 + i);
      usePlayback.setState({ playheadUs, playing: true });
      const target = timelineTargetAt(scroller, 20, 40 + playheadUs / 1e5 - 5_900, 30, 'image');
      expect(target).toMatchObject({ atUs: playheadUs });
    }
    expect(snapTargets).toHaveBeenCalledTimes(1);
    // An edit (a new revision) builds them again, once.
    useProject.setState({ snapshot: { ...snap, project: longProject() } });
    timelineTargetAt(scroller, 20, 300, 30, 'image');
    timelineTargetAt(scroller, 20, 400, 30, 'image');
    expect(snapTargets).toHaveBeenCalledTimes(2);
    usePlayback.setState({ playing: false });
  });

  it('the lanes scroll while media is held near their edge over them, never from the rest of the window', () => {
    const scroller = row({}, 400, 300);
    Object.defineProperty(scroller, 'clientWidth', { value: 1240 });
    registerDropTarget(() => null, scroller);
    const frames: FrameRequestCallback[] = [];
    const raf = vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => frames.push(cb));
    const tick = () => frames.splice(0).forEach((cb) => cb(0));
    let pointer = { x: 1232, y: 500 };
    const scrolled = vi.fn();
    const stop = dropAutoscroll(() => pointer, scrolled);
    tick();
    tick();
    expect(scroller.scrollLeft).toBeGreaterThan(0);
    expect(scrolled).toHaveBeenCalled();
    // Over the library above the timeline, as near its right edge: the lanes stay put.
    const left = scroller.scrollLeft;
    pointer = { x: 1232, y: 100 };
    tick();
    tick();
    expect(scroller.scrollLeft).toBe(left);
    stop();
    raf.mockRestore();
  });

  it('over the gutter the lanes stay put; just past it they scroll left', () => {
    const scroller = row({}, 400, 300);
    Object.defineProperty(scroller, 'clientWidth', { value: 1240 });
    scroller.scrollLeft = 500;
    registerDropTarget(() => null, scroller);
    const frames: FrameRequestCallback[] = [];
    const raf = vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => frames.push(cb));
    const tick = () => frames.splice(0).forEach((cb) => cb(0));
    // A row pulled straight down from the library, which sits above the 40 px gutter.
    let pointer = { x: 20, y: 500 };
    const stop = dropAutoscroll(() => pointer, vi.fn());
    tick();
    tick();
    expect(scroller.scrollLeft).toBe(500);
    // Into the lanes' left edge zone: now they scroll left.
    pointer = { x: 52, y: 500 };
    tick();
    tick();
    expect(scroller.scrollLeft).toBeLessThan(500);
    stop();
    raf.mockRestore();
  });

  /** The project after one more step: at `revision`, `label` on top of its history. */
  const after = (revision: number, label: string) => {
    const snapshot = fixtureSnapshot();
    snapshot.history = { ...snapshot.history, revision, undoLabel: label };
    return { ...fixturePatch(snapshot), baseRevision: revision - 1 };
  };
  const sound = (): Asset => ({
    ...fixtureSnapshot().project.assets.find((a) => a.kind === 'audio')!,
    id: 'pop',
    path: '/pop.wav',
    name: 'pop.wav',
    durationUs: 400_000,
  });

  it('the toast’s Undo after one sound let go on the Sounds lane takes it back', async () => {
    vi.mocked(bridge.importMedia).mockResolvedValue({ assets: [sound()], skipped: [] });
    vi.mocked(library.place).mockResolvedValue({ patch: after(4, 'media.sound.add'), unplaced: [] });
    await importFiles(['/pop.wav'], { lane: 'sound', trackId: null, atUs: 2_000_000, row: 'sounds' });
    await vi.waitFor(() => expect(useToasts.getState().toasts).toHaveLength(1));
    const [added] = useToasts.getState().toasts;
    expect(added.text).toBe('Added 1 file');
    vi.mocked(bridge.undo).mockResolvedValue(after(5, 'text.add'));
    added.actions![0].run();
    await vi.waitFor(() => expect(bridge.undo).toHaveBeenCalledTimes(1));
  });

  it('the toast’s Undo after videos and a sound let go together takes back both steps, newest first', async () => {
    const broll = fixtureSnapshot().project.assets[5];
    const clip: Asset = { ...broll, id: 'n1', path: '/n1.mp4', name: 'n1.mp4' };
    vi.mocked(bridge.importMedia).mockResolvedValue({ assets: [clip, sound()], skipped: [] });
    vi.mocked(bridge.apply).mockResolvedValue(after(4, 'media.import'));
    vi.mocked(library.place).mockResolvedValue({ patch: after(5, 'media.sound.add'), unplaced: [] });
    await importFiles(['/n1.mp4', '/pop.wav'], { lane: 'picture', trackId: null, atUs: 2_000_000 });
    await vi.waitFor(() => expect(useToasts.getState().toasts).toHaveLength(1));
    // Undoing the sound leaves the videos' step on top, then that goes too.
    vi.mocked(bridge.undo)
      .mockResolvedValueOnce(after(6, 'media.import'))
      .mockResolvedValueOnce(after(7, 'text.add'));
    useToasts.getState().toasts[0].actions![0].run();
    await vi.waitFor(() => expect(bridge.undo).toHaveBeenCalledTimes(2));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(bridge.undo).toHaveBeenCalledTimes(2);
  });

  it('the toast’s Undo knows each step by its own answer, not by what the copy shows when it settles', async () => {
    vi.mocked(bridge.importMedia).mockResolvedValue({ assets: [sound()], skipped: [] });
    vi.mocked(library.place).mockResolvedValue({ patch: after(4, 'media.sound.add'), unplaced: [] });
    // The copy hasn't taken the answer in yet when the step settles (it still shows revision 3, "Add text").
    const send = useProject.getState().send;
    useProject.setState({
      send: async (task) => {
        await task();
        return true;
      },
    });
    try {
      await importFiles(['/pop.wav'], { lane: 'sound', trackId: null, atUs: 2_000_000, row: 'sounds' });
      await vi.waitFor(() => expect(useToasts.getState().toasts).toHaveLength(1));
    } finally {
      useProject.setState({ send });
    }
    useProject.setState({ snapshot: { ...fixtureSnapshot(), history: after(4, 'media.sound.add').history } });
    vi.mocked(bridge.undo).mockResolvedValue(after(5, 'text.add'));
    useToasts.getState().toasts[0].actions![0].run();
    await vi.waitFor(() => expect(bridge.undo).toHaveBeenCalledTimes(1));
  });

  it('files that got no layer are named with why; a refused step stops the rest; nothing added says nothing', async () => {
    const late = { ...sound(), id: 'late', name: 'late.wav' };
    vi.mocked(bridge.importMedia).mockResolvedValue({ assets: [sound(), late], skipped: [] });
    vi.mocked(library.place).mockResolvedValue({
      patch: after(4, 'media.import'),
      unplaced: [{ name: 'late.wav', reason: { code: 'model.sound.noRoom', params: {} } }],
    });
    await importFiles(['/pop.wav', '/late.wav'], { lane: 'sound', trackId: null, atUs: 2_000_000, row: 'sounds' });
    await vi.waitFor(() => expect(useToasts.getState().toasts).toHaveLength(1));
    expect(useToasts.getState().toasts[0].text).toBe(
      'Added 2 files, 1 went to the library only: a sound has to start before the video ends.',
    );
    useToasts.setState({ toasts: [] });
    // Music refused (its own toast says why): the files after it don't go in.
    const song: Asset = { ...sound(), id: 'lofi', path: '/lofi.mp3', name: 'lofi.mp3', durationUs: 60_000_000 };
    vi.mocked(bridge.importMedia).mockResolvedValue({ assets: [song, late], skipped: [] });
    vi.mocked(library.addMusic).mockRejectedValue({ code: 'model.music.noRoom', params: {} });
    await importFiles(['/lofi.mp3', '/late.wav'], { lane: 'sound', trackId: null, atUs: 2_000_000, row: 'music' });
    await vi.waitFor(() => expect(library.addMusic).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(useToasts.getState().toasts.map((t) => t.text)).not.toContain('Added lofi.mp3 as music, 1 file to the library');
    useToasts.setState({ toasts: [] });
    // Files too short to place: nothing goes in, and no "Added" is said.
    const blip: Asset = { ...fixtureSnapshot().project.assets[5], id: 'b', path: '/b.mp4', name: 'b.mp4', durationUs: 5_000 };
    vi.mocked(bridge.importMedia).mockResolvedValue({ assets: [blip], skipped: [] });
    await importFiles(['/b.mp4']);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(useToasts.getState().toasts.map((t) => t.text).filter((text) => text.startsWith('Added'))).toEqual([]);
  });

  it('a read-only project gives no lane refusal: nothing goes in anywhere', () => {
    useUi.setState({ pxPerSecond: 10, snapping: false });
    useProject.setState({ snapshot: { ...fixtureSnapshot(), readOnly: 'From a newer Klipolo' } as never });
    const scroller = lanes();
    registerDropTarget(
      (cx, cy, kind) => timelineTargetAt(scroller, 20, cx, cy, kind),
      scroller,
      (cx, cy) => timelineLaneAt(scroller, 20, cx, cy),
    );
    expect(dropRefusal(x(3), 30, 'image')).toBeNull();
    expect(dropRefusal(x(3), 80, 'audio')).toBeNull();
  });
});
