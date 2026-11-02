/**
 * The page's perf readout (`KLIPOLO_PAGE_PERF=1`), one log line at a time:
 * - a project's first render: from the press to its snapshot, then to its first commit and first paint;
 * - each scroll and zoom of the timeline, once it settles: its frames and what rendered;
 * - while playing, every 5 s and once it stops: playhead updates, commits and renders a second, by panel, and frames.
 * Frames are timed by a frame loop that runs only while one of these is being measured.
 */
import type { Snapshot } from '../bindings/Snapshot';
import { usePlayback } from '../state/playback';
import { useProject } from '../state/project';
import { useUi } from '../state/ui';
import { frameStats, framesLine } from './frames';
import { onCommit, renderTotals, type RenderTotals } from './renders';

export interface ReadoutDeps {
  log: (line: string) => void;
  now?: () => number;
  frame?: (cb: () => void) => void;
}

/** A gesture has settled once nothing moved for this long (ms). */
const SETTLE_MS = 300;
/** How often a line is written while playing (ms). */
const PLAYING_EVERY_MS = 5000;
/** Panels named in a line, most renders first, at most this many. */
const TOP_PANELS = 6;

/** What rendered between `from` and `to`: in all, or a second over `per` seconds. */
function rendersLine(from: RenderTotals, to: RenderTotals, per?: number): string {
  const rate = (n: number) => Math.round(n / (per ?? 1));
  const panels = Object.entries(to.byPanel)
    .map(([panel, n]): [string, number] => [panel, rate(n - (from.byPanel[panel] ?? 0))])
    .filter(([, n]) => n > 0)
    .sort((a, b) => b[1] - a[1])
    .slice(0, TOP_PANELS)
    .map(([panel, n]) => `${panel} ${n}`)
    .join(', ');
  const counts =
    `${rate(to.commits - from.commits)} commits, ${rate(to.renders - from.renders)} renders` +
    (per === undefined ? '' : ' a second');
  return panels ? `${counts} (${panels})` : counts;
}

/** The most rendered components between `from` and `to` (a second over `per`), when names are known (development). */
function namesLine(from: RenderTotals, to: RenderTotals, per = 1): string {
  const top = Object.entries(to.byName)
    .map(([name, n]): [string, number] => [name, Math.round((n - (from.byName[name] ?? 0)) / per)])
    .filter(([name, n]) => n > 0 && name.length > 2)
    .sort((a, b) => b[1] - a[1])
    .slice(0, 5);
  return top.length > 0 ? `; most: ${top.map(([name, n]) => `${name} ${n}`).join(', ')}` : '';
}

const layersOf = (s: Snapshot) => s.project.timeline.tracks.reduce((n, t) => n + t.items.length, 0);
const openingOf = (s: Snapshot | null) => (s ? `${s.project.id}:${s.opening ?? ''}` : null);

/** Starts the readout; the function it returns stops it. */
export function startPagePerf({
  log,
  now = () => performance.now(),
  frame = (cb) => requestAnimationFrame(cb),
}: ReadoutDeps): () => void {
  let stopped = false;
  const offs: (() => void)[] = [];

  // The frame loop: each frame's length goes to whoever measures; it stops once nobody does.
  const measuring = new Set<(delta: number, at: number) => void>();
  let looping = false;
  let lastFrame = 0;
  const tick = () => {
    if (stopped || measuring.size === 0) {
      looping = false;
      return;
    }
    const at = now();
    const delta = at - lastFrame;
    lastFrame = at;
    for (const m of [...measuring]) m(delta, at);
    if (measuring.size > 0) frame(tick);
    else looping = false;
  };
  const measure = (m: (delta: number, at: number) => void) => {
    measuring.add(m);
    if (!looping) {
      looping = true;
      lastFrame = now();
      frame(tick);
    }
    return () => measuring.delete(m);
  };

  // Timeline gestures: a scroll of its lanes or a change of zoom, until nothing moved for SETTLE_MS.
  let gesture: { kind: 'scroll' | 'zoom'; from: RenderTotals; deltas: number[]; last: number; end: () => void } | null =
    null;
  const moved = (kind: 'scroll' | 'zoom') => {
    if (stopped) return;
    if (gesture) {
      gesture.last = now();
      if (kind === 'zoom') gesture.kind = 'zoom';
      return;
    }
    const g = { kind, from: renderTotals(), deltas: [] as number[], last: now(), end: () => {} };
    gesture = g;
    g.end = measure((delta, at) => {
      g.deltas.push(delta);
      if (at - g.last < SETTLE_MS) return;
      g.end();
      gesture = null;
      const to = renderTotals();
      log(`timeline ${g.kind}: ${framesLine(frameStats(g.deltas))}; ${rendersLine(g.from, to)}${namesLine(g.from, to)}`);
    });
  };
  const onScroll = (e: Event) => {
    if (e.target instanceof Element && e.target.closest('.timeline')) moved('scroll');
  };
  document.addEventListener('scroll', onScroll, { capture: true, passive: true });
  offs.push(() => document.removeEventListener('scroll', onScroll, { capture: true }));
  offs.push(useUi.subscribe((s, was) => s.pxPerSecond !== was.pxPerSecond && moved('zoom')));

  // Playing: playhead updates, commits, renders and frames, every PLAYING_EVERY_MS and for the whole run.
  let run: {
    start: number;
    from: RenderTotals;
    updates: number;
    deltas: number[];
    window: { start: number; from: RenderTotals; updates: number; deltas: number[] };
    end: () => void;
  } | null = null;
  const playingLine = (word: string, start: number, from: RenderTotals, updates: number, deltas: number[]) => {
    const secs = (now() - start) / 1000;
    const per = Math.max(secs, 0.001);
    const to = renderTotals();
    return (
      `${word} ${Math.round(secs)} s: ${Math.round(updates / per)} playhead updates, ${rendersLine(from, to, per)}; ` +
      `${framesLine(frameStats(deltas))}${namesLine(from, to, per)}`
    );
  };
  offs.push(
    usePlayback.subscribe((s, was) => {
      if (s.playing && !was.playing && !run) {
        const from = renderTotals();
        const r = {
          start: now(),
          from,
          updates: 0,
          deltas: [] as number[],
          window: { start: now(), from, updates: 0, deltas: [] as number[] },
          end: () => {},
        };
        run = r;
        r.end = measure((delta, at) => {
          r.deltas.push(delta);
          r.window.deltas.push(delta);
          if (at - r.window.start < PLAYING_EVERY_MS) return;
          const w = r.window;
          log(playingLine('playing', w.start, w.from, w.updates, w.deltas));
          r.window = { start: at, from: renderTotals(), updates: 0, deltas: [] };
        });
      } else if (run && s.playing && s.playheadUs !== was.playheadUs) {
        run.updates += 1;
        run.window.updates += 1;
      } else if (run && !s.playing && was.playing) {
        const r = run;
        run = null;
        r.end();
        const elements = document.getElementsByTagName('*').length;
        log(`${playingLine('played', r.start, r.from, r.updates, r.deltas)}; ${elements} elements on the page`);
      }
    }),
  );

  // A project's first render: the press (a create, open or duplicate), its snapshot, its first commit and paint.
  let pressedAt: number | null = null;
  let waiting: (() => void) | null = null;
  offs.push(
    useProject.subscribe((s, was) => {
      if (s.switching && !was.switching) pressedAt = now();
      const opening = openingOf(s.snapshot);
      if (!s.snapshot || opening === openingOf(was.snapshot)) return;
      const snapshot = s.snapshot;
      const at = now();
      const asked = pressedAt;
      pressedAt = null;
      const from = renderTotals();
      waiting?.();
      const off = onCommit(() => {
        off();
        waiting = null;
        const committed = now();
        const renders = renderTotals().renders - from.renders;
        frame(() =>
          setTimeout(() => {
            if (stopped) return;
            const { name, timeline } = snapshot.project;
            const head = `project open "${name}" (${timeline.main.length} clips, ${layersOf(snapshot)} layers): `;
            const press = asked === null ? '' : `snapshot ${Math.round(at - asked)} ms after the press, `;
            log(
              `${head}${press}first render ${Math.round(committed - at)} ms after it (${renders} renders), ` +
                `first paint ${Math.round(now() - at)} ms after it`,
            );
          }),
        );
      });
      waiting = off;
    }),
  );

  return () => {
    stopped = true;
    waiting?.();
    gesture?.end();
    run?.end();
    for (const off of offs) off();
  };
}
