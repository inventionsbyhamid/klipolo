import { renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Asset } from '../../bindings/Asset';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { longProject } from '../../test/longProject';

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn(), importMedia: vi.fn() },
  mediaUrl: (p: string) => p,
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
const { registerDropTarget, timelineTargetAt, dropAutoscroll } = await import('../timeline/dropTarget');

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 2_000_000 });
  useToasts.setState({ toasts: [] });
  vi.mocked(bridge.apply).mockReset().mockResolvedValue(fixturePatch(fixtureSnapshot()));
  vi.mocked(bridge.importMedia).mockReset();
  vi.mocked(bridge.undo).mockReset();
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
    // The main track takes every kind; files from Finder (kind unknown) take the lane's.
    expect(at(24, 80, 'audio')).toEqual({ lane: 'main', index: 1 });
    expect(at(3, 160, null)).toMatchObject({ lane: 'sound' });
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
    await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    const ops = vi.mocked(bridge.apply).mock.calls[0][0].ops;
    // Placed by the model on a track that takes it (a new one here), never on the locked overlay track.
    expect(ops).toMatchObject([{ type: 'addAssets' }, { type: 'addItem', track: { type: 'auto' } }]);
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
});
