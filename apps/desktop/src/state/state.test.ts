import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixturePatch, fixtureSnapshot } from '../test/fixtures';

vi.mock('../bridge/commands', () => ({
  bridge: {
    apply: vi.fn(),
    undo: vi.fn(),
    redo: vi.fn(),
    createProject: vi.fn(),
    openProject: vi.fn(),
    closeProject: vi.fn(),
    importMedia: vi.fn(),
    snapshot: vi.fn(),
  },
  mediaUrl: (p: string) => p,
}));
vi.mock('../bridge/engine', () => ({
  engine: {
    seek: vi.fn(() => Promise.resolve()),
    play: vi.fn(() => Promise.resolve()),
    pause: vi.fn(() => Promise.resolve(1_500_000)),
  },
}));

const { bridge } = await import('../bridge/commands');
const { engine } = await import('../bridge/engine');
const { useProject } = await import('./project');
const { useToasts } = await import('./toasts');
const { useUi } = await import('./ui');
const { usePlayback } = await import('./playback');
const { clockAnchor, holdClock } = await import('./playClock');
const { useSelection } = await import('./selection');

describe('project store', () => {
  beforeEach(() => {
    vi.mocked(bridge.apply).mockReset();
    useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
    useToasts.setState({ toasts: [] });
  });

  it('replaces the snapshot after a successful edit', async () => {
    const next = fixtureSnapshot();
    next.project.name = 'Renamed';
    vi.mocked(bridge.apply).mockResolvedValue(fixturePatch(next));
    expect(
      await useProject.getState().apply('editor.rename.undo', [{ type: 'rename', name: 'Renamed' }]),
    ).toBe(true);
    expect(useProject.getState().snapshot?.project.name).toBe('Renamed');
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'editor.rename.undo',
      ops: [{ type: 'rename', name: 'Renamed' }],
    });
    // A label's values go with it: history words them in the language shown.
    const settings = next.project.settings;
    await useProject
      .getState()
      .apply('format.add.undo', [{ type: 'setSettings', settings }], { ratio: '9:16' });
    expect(bridge.apply).toHaveBeenLastCalledWith({
      label: 'format.add.undo',
      params: { ratio: '9:16' },
      ops: [{ type: 'setSettings', settings }],
    });
  });


  it('keeps the old snapshot and shows the error when an edit fails', async () => {
    vi.mocked(bridge.apply).mockRejectedValue(new Error('Couldn’t save the project: disk full'));
    expect(await useProject.getState().apply('editor.rename.undo', [{ type: 'rename', name: 'X' }])).toBe(
      false,
    );
    expect(useProject.getState().snapshot?.project.name).toBe('Billing tutorial');
    expect(useToasts.getState().toasts[0].text).toContain('disk full');
  });

  it('says once that a newer project is read-only, then edits quietly do nothing', async () => {
    const notice = 'This project is read-only because it was made with a newer version of Klipolo.';
    vi.mocked(bridge.undo).mockClear();
    // Rust sends the notice as a code; the UI words it.
    vi.mocked(bridge.openProject).mockResolvedValue({ ...fixtureSnapshot(), readOnly: 'model.readOnly' });
    expect(await useProject.getState().open('/Movies/Klipolo/project.klipolo')).toBe(true);
    expect(await useProject.getState().apply('editor.rename.undo', [{ type: 'rename', name: 'X' }])).toBe(
      false,
    );
    await useProject.getState().undo();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(bridge.undo).not.toHaveBeenCalled();
    expect(useToasts.getState().toasts.map((t) => [t.kind, t.text])).toEqual([['info', notice]]);
  });

  it('runs edits one at a time, in order', async () => {
    const order: string[] = [];
    let release!: () => void;
    vi.mocked(bridge.apply)
      .mockImplementationOnce(async () => {
        order.push('first-start');
        await new Promise<void>((resolve) => (release = resolve));
        order.push('first-end');
        return fixturePatch(fixtureSnapshot());
      })
      .mockImplementationOnce(async () => {
        order.push('second');
        return fixturePatch(fixtureSnapshot());
      });
    const a = useProject.getState().apply('editor.rename.undo', [{ type: 'rename', name: 'A' }]);
    const b = useProject.getState().apply('editor.rename.undo', [{ type: 'rename', name: 'B' }]);
    await vi.waitFor(() => expect(order).toEqual(['first-start']));
    release();
    await Promise.all([a, b]);
    expect(order).toEqual(['first-start', 'first-end', 'second']);
  });

  it('opens a project at the start with nothing selected, in its own format', async () => {
    vi.mocked(bridge.openProject).mockResolvedValue(fixtureSnapshot());
    usePlayback.getState().seek(12 * 1_000_000);
    useSelection.getState().select('title');
    useUi.getState().setPreviewFormat('portrait');
    expect(await useProject.getState().open('/Movies/Klipolo/project.klipolo')).toBe(true);
    expect(usePlayback.getState().playheadUs).toBe(0);
    expect(useSelection.getState().selectedId).toBeNull();
    expect(useUi.getState().previewFormat).toBeNull();
  });

  it('applies a patch made on its revision and asks for the whole project otherwise', async () => {
    const snapshot = fixtureSnapshot();
    useProject.setState({ snapshot, pending: 0 });
    vi.mocked(bridge.apply).mockResolvedValueOnce({
      baseRevision: snapshot.history.revision,
      history: { ...snapshot.history, revision: snapshot.history.revision + 1 },
      patch: { name: 'Renamed', clips: [], items: [] },
    });
    await useProject.getState().apply('editor.rename.undo', [{ type: 'rename', name: 'Renamed' }]);
    expect(useProject.getState().snapshot!.project.name).toBe('Renamed');
    expect(useProject.getState().snapshot!.project.timeline).toBe(snapshot.project.timeline);

    vi.mocked(bridge.apply).mockResolvedValueOnce({
      baseRevision: 99,
      history: { ...snapshot.history, revision: 100 },
      patch: { clips: [], items: [] },
    });
    vi.mocked(bridge.snapshot).mockResolvedValueOnce({
      ...snapshot,
      history: { ...snapshot.history, revision: 100 },
    });
    await useProject.getState().apply('editor.rename.undo', [{ type: 'rename', name: 'X' }]);
    expect(bridge.snapshot).toHaveBeenCalledTimes(1);
    expect(useProject.getState().snapshot!.history.revision).toBe(100);
  });

  it('asks for the whole project when a patch names a clip it doesn’t have, rather than drop it', async () => {
    const snapshot = fixtureSnapshot();
    useProject.setState({ snapshot, pending: 0 });
    const history = { ...snapshot.history, revision: snapshot.history.revision + 1 };
    vi.mocked(bridge.apply).mockResolvedValueOnce({
      baseRevision: snapshot.history.revision,
      history,
      patch: { main: ['a', 'ghost', 'b'], clips: [], items: [] },
    });
    vi.mocked(bridge.snapshot)
      .mockClear()
      .mockResolvedValueOnce({ ...snapshot, history });
    expect(await useProject.getState().apply('editor.rename.undo', [{ type: 'rename', name: 'X' }])).toBe(
      true,
    );
    expect(bridge.snapshot).toHaveBeenCalledTimes(1);
    expect(useProject.getState().snapshot!.history.revision).toBe(history.revision);
  });

  it('skips empty edits', async () => {
    expect(await useProject.getState().apply('editor.rename.undo', [])).toBe(true);
    expect(bridge.apply).not.toHaveBeenCalled();
  });
});

describe('ui store', () => {
  it('clamps timeline zoom', () => {
    useUi.getState().setPxPerSecond(10_000);
    expect(useUi.getState().pxPerSecond).toBe(2400);
    useUi.getState().setPxPerSecond(0);
    expect(useUi.getState().pxPerSecond).toBe(0.02);
  });
});

describe('playback follows the engine', () => {
  beforeEach(() => usePlayback.setState({ playheadUs: 1_000_000, playing: false, seq: 4 }));

  it('plays from the playhead and follows drawn frames of the current seek', () => {
    usePlayback.getState().toggle();
    expect(engine.play).toHaveBeenCalledWith(1_000_000, 1, 4);
    usePlayback.getState().onFrame({ timeUs: 1_033_333, seq: 4, playback: true });
    expect(usePlayback.getState().playheadUs).toBe(1_033_333);
    usePlayback.getState().onFrame({ timeUs: 9_000_000, seq: 3, playback: true }); // rendered before the latest seek
    usePlayback.getState().onFrame({ timeUs: 9_000_000, seq: 4, playback: false }); // a still
    expect(usePlayback.getState().playheadUs).toBe(1_033_333);
  });

  it('a seek while playing ignores frames until the engine applies it', () => {
    usePlayback.setState({ playing: true });
    usePlayback.getState().seek(5_000_000);
    usePlayback.getState().onFrame({ timeUs: 1_100_000, seq: 4, playback: true });
    expect(usePlayback.getState().playheadUs).toBe(5_000_000);
  });

  it('pauses where the engine stopped and stops on ended', async () => {
    usePlayback.setState({ playing: true });
    usePlayback.getState().pause();
    await vi.waitFor(() => expect(usePlayback.getState().playheadUs).toBe(1_500_000));
    usePlayback.setState({ playing: true });
    usePlayback.getState().onState({ playing: false, atUs: 29_966_667, reason: 'ended', rate: 1 });
    expect(usePlayback.getState()).toMatchObject({ playing: false, playheadUs: 29_966_667 });
  });

  it('shuttles: L plays, L again doubles, J reverses, J again doubles', async () => {
    usePlayback.setState({ playheadUs: 5_000_000, playing: false, rate: 1 });
    usePlayback.getState().shuttle(1);
    expect(engine.play).toHaveBeenLastCalledWith(5_000_000, 1, 4);
    usePlayback.getState().onState({ playing: true, atUs: 5_000_000, reason: 'play', rate: 1 });
    usePlayback.getState().shuttle(1);
    expect(engine.play).toHaveBeenLastCalledWith(expect.any(Number), 2, 5);
    usePlayback.getState().shuttle(-1);
    expect(engine.play).toHaveBeenLastCalledWith(expect.any(Number), -1, 6);
  });

  it('after a shuttle press, frames still on their way from the run before are dropped', () => {
    usePlayback.setState({ playheadUs: 5_000_000, playing: true, rate: 1 });
    holdClock(5_000_000);
    usePlayback.getState().shuttle(-1);
    // Drawn ahead at 1× before the press (sequence 4): it must not jump the playhead forward.
    usePlayback.getState().onFrame({ timeUs: 5_100_000, seq: 4, playback: true, atMs: 0 });
    expect(usePlayback.getState().playheadUs).toBe(5_000_000);
    expect(clockAnchor().running).toBe(false);
  });

  it('J at the start does nothing (no badge, no announcement)', () => {
    usePlayback.setState({ playheadUs: 0, playing: false, rate: 1 });
    vi.mocked(engine.play).mockClear();
    usePlayback.getState().shuttle(-1);
    expect(engine.play).not.toHaveBeenCalled();
    expect(usePlayback.getState()).toMatchObject({ playing: false, rate: 1 });
  });

  it('backward, the clock is anchored where the frame is drawn: its last µs', () => {
    holdClock(2_000_000);
    usePlayback.setState({ playheadUs: 2_000_000, playing: true, rate: -1 });
    // Frame 30 at 30 fps (1 000 000 to 1 033 332) is drawn as the clock enters it from above.
    usePlayback.getState().onFrame({ timeUs: 1_000_000, seq: 4, playback: true, atMs: 100 });
    expect(usePlayback.getState().playheadUs).toBe(1_000_000);
    expect(clockAnchor()).toMatchObject({ us: 1_033_332, atMs: 100, running: true, rate: -1 });
  });

  it('a backward run that starts mid-frame starts from where it was held, not ahead of it', () => {
    // L then J at 1.01 s, inside frame 30 (1 000 000 to 1 033 332): the first frame drawn is frame 30.
    holdClock(1_010_000);
    usePlayback.setState({ playheadUs: 1_010_000, playing: true, rate: -1 });
    usePlayback.getState().onFrame({ timeUs: 1_000_000, seq: 4, playback: true, atMs: 100 });
    expect(clockAnchor()).toMatchObject({ us: 1_010_000, atMs: 100, running: true, rate: -1 });
  });

  it("a seek right after pausing isn't undone by the engine's stop report", async () => {
    usePlayback.setState({ playing: true });
    usePlayback.getState().pause();
    usePlayback.getState().seek(7_000_000);
    usePlayback.getState().onState({ playing: false, atUs: 1_500_000, reason: 'pause', rate: 1 });
    await Promise.resolve();
    expect(usePlayback.getState().playheadUs).toBe(7_000_000);
  });
});
