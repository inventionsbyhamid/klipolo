import { act, fireEvent, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot, revisionPatch } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { bridge } = await import('../../bridge/commands');
const { engine } = await import('../../bridge/engine');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');

const PLAYHEAD = 25_000_000;
const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));
const block = (name: RegExp) => screen.getByRole('button', { name });
const edge = (el: HTMLElement, which: 'start' | 'end') =>
  el.querySelector<HTMLElement>(`[data-trim="${which}"]`)!;
const seeks = () => vi.mocked(engine.seek).mock.calls;
/** The engine shows `us` once the newest seek names it: waited for, not timed (it goes out a frame later). */
const shows = (us: number) => vi.waitFor(() => expect(seeks().at(-1)?.[0]).toBe(us));

const press = (el: HTMLElement, x: number) =>
  fireEvent.pointerDown(el, { pointerId: 1, button: 0, clientX: x, clientY: 10 });
const move = (el: HTMLElement, x: number) =>
  fireEvent.pointerMove(el, { pointerId: 1, clientX: x, clientY: 10 });
const release = (el: HTMLElement, x: number) =>
  fireEvent.pointerUp(el, { pointerId: 1, clientX: x, clientY: 10 });

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  usePlayback.setState({ playheadUs: PLAYHEAD, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: true });
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
  vi.mocked(engine.seek).mockClear();
});

describe("a trim shows its edge's frame", () => {
  it("a main clip's end trim shows its new last frame each frame, then the playhead's on release", async () => {
    renderWithProviders(<Timeline />);
    const a = block(/^screen\.mp4/);
    // 40 px/s at 30 fps: a ends at 20 s; −40 px ends it at 19 s, whose last frame starts a frame earlier.
    press(edge(a, 'end'), 799);
    move(a, 759);
    await shows(18_966_667);
    move(a, 719);
    await shows(17_966_667);
    release(a, 719);
    await shows(PLAYHEAD);
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'timeline.trim.clip.undo',
      ops: [{ type: 'trimClip', id: 'a', edge: 'end', sourceUs: 18_000_000 }],
    });
    // Each is a seek of its own, newer than the one before, as the playhead's are.
    const seqs = seeks().map(([, seq]) => seq);
    expect(seqs).toEqual([...seqs].sort((x, y) => x - y));
    expect(new Set(seqs).size).toBe(seqs.length);
    expect(seqs.at(-1)).toBe(usePlayback.getState().seq);
  });

  it("a main clip's start trim shows its start: its new first frame, in the dry run", async () => {
    renderWithProviders(<Timeline />);
    const b = block(/^broll\.mp4/);
    // b runs 20 to 30 s; its start 1 s later takes a second off its front, and it still starts at 20 s.
    press(edge(b, 'start'), 800);
    move(b, 840);
    await shows(20_000_000);
    await vi.waitFor(() =>
      expect(vi.mocked(engine.preview).mock.calls.at(-1)?.[2]).toEqual([
        { type: 'trimClip', id: 'b', edge: 'start', sourceUs: 1_000_000 },
      ]),
    );
    release(b, 840);
    await shows(PLAYHEAD);
  });

  it("a layer's trim shows its own edge: its first frame from the start, its last from the end", async () => {
    renderWithProviders(<Timeline />);
    // The title runs 1 to 4 s (40 to 160 px).
    let title = block(/Set up billing/);
    press(edge(title, 'start'), 40);
    move(title, 80);
    await shows(2_000_000);
    release(title, 80);
    await shows(PLAYHEAD);
    await frames();
    title = block(/Set up billing/);
    press(edge(title, 'end'), 159);
    move(title, 199);
    await shows(4_966_667);
    release(title, 199);
    await shows(PLAYHEAD);
  });

  it("Escape shows the playhead's frame again and commits nothing", async () => {
    renderWithProviders(<Timeline />);
    const a = block(/^screen\.mp4/);
    press(edge(a, 'end'), 799);
    move(a, 759);
    await shows(18_966_667);
    fireEvent.keyDown(window, { key: 'Escape' });
    await shows(PLAYHEAD);
    release(a, 759);
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it("a cancelled trim (the pointer lost) shows the playhead's frame again", async () => {
    renderWithProviders(<Timeline />);
    const title = block(/Set up billing/);
    press(edge(title, 'end'), 159);
    move(title, 199);
    await shows(4_966_667);
    fireEvent.pointerCancel(title, { pointerId: 1 });
    await shows(PLAYHEAD);
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it("a trim taken back to where it began shows the playhead's frame again on release", async () => {
    renderWithProviders(<Timeline />);
    const a = block(/^screen\.mp4/);
    press(edge(a, 'end'), 799);
    move(a, 759);
    await shows(18_966_667);
    move(a, 799);
    await shows(19_966_667);
    release(a, 799);
    await shows(PLAYHEAD);
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it('the playhead and its timecode never move during the trim', async () => {
    renderWithProviders(<Timeline />);
    const moved: number[] = [];
    const off = usePlayback.subscribe((s, prev) => {
      if (s.playheadUs !== prev.playheadUs || s.playing !== prev.playing) moved.push(s.playheadUs);
    });
    const a = block(/^screen\.mp4/);
    press(edge(a, 'end'), 799);
    for (const x of [759, 719, 679]) {
      move(a, x);
      await frames();
    }
    await shows(16_966_667);
    release(a, 679);
    await shows(PLAYHEAD);
    await frames();
    off();
    expect(moved).toEqual([]);
    expect(usePlayback.getState().playheadUs).toBe(PLAYHEAD);
  });

  it("a move isn't a trim: the preview keeps the playhead's frame", async () => {
    renderWithProviders(<Timeline />);
    // The playhead is inside b, so pressing it doesn't move the playhead either.
    const b = block(/^broll\.mp4/);
    press(b, 1000);
    move(b, 1100);
    await frames();
    await frames();
    expect(engine.seek).not.toHaveBeenCalled();
    fireEvent.keyDown(window, { key: 'Escape' });
    release(b, 1100);
  });

  it("while playing, the picture stays playback's: a trim never seeks", async () => {
    usePlayback.setState({ playing: true });
    renderWithProviders(<Timeline />);
    const a = block(/^screen\.mp4/);
    press(edge(a, 'end'), 799);
    move(a, 759);
    await frames();
    await frames();
    release(a, 759);
    await frames();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(engine.seek).not.toHaveBeenCalled();
  });
});

describe('a trim ended from elsewhere goes back to the playhead', () => {
  it('a change from elsewhere merged mid-trim', async () => {
    renderWithProviders(<Timeline />);
    const a = block(/^screen\.mp4/);
    press(edge(a, 'end'), 799);
    move(a, 759);
    await shows(18_966_667);
    // Chat's change (3 to 4) merges mid-trim: the trim stops where it is.
    await act(() => useProject.getState().adopt(revisionPatch(4)));
    await shows(PLAYHEAD);
    // It draws and shows no more.
    move(a, 719);
    await frames();
    expect(seeks().at(-1)?.[0]).toBe(PLAYHEAD);
    release(a, 719);
    await frames();
  });

  it('a zoom mid-trim', async () => {
    renderWithProviders(<Timeline />);
    const a = block(/^screen\.mp4/);
    press(edge(a, 'end'), 799);
    move(a, 759);
    await shows(18_966_667);
    act(() => useUi.setState({ pxPerSecond: 80 }));
    await shows(PLAYHEAD);
    release(a, 759);
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it('the pointer capture lost mid-trim', async () => {
    renderWithProviders(<Timeline />);
    const a = block(/^screen\.mp4/);
    press(edge(a, 'end'), 799);
    move(a, 759);
    await shows(18_966_667);
    fireEvent.lostPointerCapture(a, { pointerId: 1 });
    await shows(PLAYHEAD);
    release(a, 759);
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
  });
});

