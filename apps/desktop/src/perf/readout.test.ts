import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { frameStats } from './frames';
import { renderTotals, tallyCommit } from './renders';

vi.mock('../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
const { usePlayback } = await import('../state/playback');
const { useProject } = await import('../state/project');
const { useUi } = await import('../state/ui');
const { readoutOnLine, startPagePerf } = await import('./readout');

/** A fiber as React commits it: `flags & 1` when it rendered, `subtreeFlags & 1` when something under it did. */
interface F {
  tag: number;
  type?: unknown;
  memoizedProps?: Record<string, unknown>;
  flags: number;
  subtreeFlags: number;
  child: F | null;
  sibling: F | null;
}
const fiber = (tag: number, flags: number, children: F[] = [], props: Record<string, unknown> = {}, type?: unknown): F => {
  children.forEach((c, i) => (c.sibling = children[i + 1] ?? null));
  const subtreeFlags = children.reduce((all, c) => all | c.flags | c.subtreeFlags, 0);
  return { tag, type, memoizedProps: props, flags, subtreeFlags, child: children[0] ?? null, sibling: null };
};
function Block() {}
function Ruler() {}
function Timecode() {}
/** One commit: two blocks and the ruler rendered in the timeline, the timecode in the preview, a still block skipped. */
const commit = () =>
  fiber(3, 0, [
    fiber(5, 0, [fiber(0, 1, [], {}, Block), fiber(0, 1, [], {}, Block), fiber(0, 0, [], {}, Block), fiber(0, 1, [], {}, Ruler)], {
      className: 'panel timeline',
    }),
    fiber(5, 0, [fiber(0, 1, [], {}, Timecode)], { className: 'panel stage' }),
    fiber(5, 0, [fiber(0, 0, [], {}, Block)], { className: 'panel chat' }),
  ]);

describe('the render tally', () => {
  it('counts the components that rendered in a commit, by panel and name, and skips the rest', () => {
    const before = renderTotals();
    tallyCommit(commit());
    const after = renderTotals();
    expect(after.commits - before.commits).toBe(1);
    expect(after.renders - before.renders).toBe(4);
    expect((after.byPanel.timeline ?? 0) - (before.byPanel.timeline ?? 0)).toBe(3);
    expect((after.byPanel.preview ?? 0) - (before.byPanel.preview ?? 0)).toBe(1);
    expect(after.byPanel.chat ?? 0).toBe(before.byPanel.chat ?? 0);
    expect((after.byName.Block ?? 0) - (before.byName.Block ?? 0)).toBe(2);
    expect((after.byWhere['timeline Block'] ?? 0) - (before.byWhere['timeline Block'] ?? 0)).toBe(2);
  });
});

describe('frame stats', () => {
  it('says the median, the 95th percentile, the longest and the frames late by more than vsync jitter', () => {
    const s = frameStats([16.6, 17.4, 16.8, 55, 16.7, 34, 16.6, 16.7, 21, 16.7]);
    expect(s).toMatchObject({ frames: 10, median: 17, longest: 55, late: 3, veryLate: 1 });
    expect(s.p95).toBe(55);
    expect(frameStats([])).toMatchObject({ frames: 0, longest: 0 });
  });
});

describe('the page perf readout', () => {
  let lines: string[] = [];
  let frames: (() => void)[] = [];
  let now = 0;
  let stop = () => {};
  /** `n` frames of `len` ms (the last one `last` ms), each frame's callbacks run as it ends. */
  const run = (n: number, len = 16, last = len) => {
    for (let i = 0; i < n; i++) {
      const step = i === n - 1 ? last : len;
      now += step;
      vi.advanceTimersByTime(step);
      const due = frames;
      frames = [];
      due.forEach((f) => f());
    }
  };
  beforeEach(() => {
    vi.useFakeTimers();
    lines = [];
    frames = [];
    now = 0;
    useProject.setState({ snapshot: null, switching: false });
    usePlayback.setState({ playing: false, playheadUs: 0 });
    useUi.setState({ pxPerSecond: 40 });
    stop = startPagePerf({ log: (l) => lines.push(l), now: () => now, frame: (cb) => frames.push(cb) });
  });
  afterEach(() => {
    stop();
    vi.useRealTimers();
  });

  it('while playing, says the playhead updates, commits and renders a second, and sums up the run', () => {
    usePlayback.setState({ playing: true });
    // 5 s at 25 frames a second from the engine, a commit every fifth, two 20 ms frames of the page each.
    for (let f = 0; f < 125; f++) {
      usePlayback.setState({ playheadUs: f * 40_000 });
      if (f % 5 === 0) tallyCommit(commit());
      run(2, 20);
    }
    const playing = lines.find((l) => l.startsWith('playing'));
    expect(playing).toMatch(/^playing 5 s: 25 playhead updates, 5 commits, 20 renders a second \(timeline 15, preview 5\)/);
    expect(playing).toMatch(/250 frames, median 20 ms/);
    usePlayback.setState({ playing: false });
    expect(lines.at(-1)).toMatch(/^played 5 s: 25 playhead updates, 5 commits, 20 renders a second/);
    expect(lines.at(-1)).toMatch(/; \d+ elements on the page/);
  });

  it('a zoom of the timeline says its frames and renders once it settles', () => {
    useUi.setState({ pxPerSecond: 50 });
    tallyCommit(commit());
    run(3, 16, 40);
    useUi.setState({ pxPerSecond: 60 });
    run(2);
    expect(lines).toHaveLength(0);
    run(25);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toMatch(/^timeline zoom: \d+ frames, median 16 ms, p95 \d+ ms, longest 40 ms, 1 over 20 ms, 0 over 50 ms/);
    expect(lines[0]).toMatch(/1 commits, 4 renders \(timeline 3, preview 1\)/);
  });

  it('a scroll of the timeline is timed as a scroll', () => {
    const lanes = document.createElement('div');
    lanes.className = 'panel timeline';
    const scroller = lanes.appendChild(document.createElement('div'));
    document.body.append(lanes);
    scroller.dispatchEvent(new Event('scroll'));
    run(4);
    scroller.dispatchEvent(new Event('scroll'));
    run(30);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toMatch(/^timeline scroll: \d+ frames/);
    lanes.remove();
  });

  it("a project's first render: from the press to its snapshot, its first commit and its first paint", () => {
    useProject.setState({ switching: true });
    now += 300;
    useProject.setState({ snapshot: fixtureSnapshot(), switching: false });
    now += 120;
    tallyCommit(commit());
    run(1);
    vi.advanceTimersByTime(1);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toMatch(
      /^project open "Billing tutorial" \(2 clips, 2 layers\): snapshot 300 ms after the press, first render 120 ms after it \(4 renders\), first paint 136 ms after it$/,
    );
  });

  it('off, it measures nothing', () => {
    stop();
    lines = [];
    usePlayback.setState({ playing: true });
    run(200);
    usePlayback.setState({ playing: false });
    expect(lines).toHaveLength(0);
    expect(frames).toHaveLength(0);
  });
});

describe('the first line', () => {
  it("says the readout is on, and whether React's commits reach the count", () => {
    tallyCommit(commit());
    expect(readoutOnLine()).toBe('page perf readout on; React commits counted');
  });
});
