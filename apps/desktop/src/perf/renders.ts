/**
 * The page's renders, counted from React's commits for the perf readout: React tells a DevTools hook of every commit,
 * and each component that rendered in it carries a flag. Counted by panel (the nearest panel around it) and by name
 * (names survive in development builds only). The hook is installed only when the readout is on, before React loads
 * (`main.tsx` imports this first); otherwise nothing here runs.
 */
import { pagePerfOn } from './flag';

/** The fields of React's fibers read here. */
interface Fiber {
  tag: number;
  type?: unknown;
  memoizedProps?: unknown;
  flags: number;
  subtreeFlags: number;
  child: Fiber | null;
  sibling: Fiber | null;
}

/** Renders since the page loaded: running totals, read as differences. */
export interface RenderTotals {
  commits: number;
  renders: number;
  byPanel: Record<string, number>;
  byName: Record<string, number>;
  /** By panel and name together ("timeline Tooltip"). */
  byWhere: Record<string, number>;
}

/** React's fiber flag for a component that rendered (`PerformedWork`), and the tags of components. */
const PERFORMED_WORK = 1;
const HOST_COMPONENT = 5;
const COMPONENT_TAGS = new Set([0, 1, 11, 15]);

/** A panel's class, and what the readout calls it (the innermost wins: the inspector is in the preview). */
const PANELS: [string, string][] = [
  ['topbar', 'top bar'],
  ['library', 'library'],
  ['cap-scroll', 'captions'],
  ['stage', 'preview'],
  ['inspector', 'inspector'],
  ['timeline', 'timeline'],
  ['chat', 'chat'],
  ['playerbar', 'player bar'],
];

const totals: RenderTotals = { commits: 0, renders: 0, byPanel: {}, byName: {}, byWhere: {} };

/** A copy of the totals now. */
export function renderTotals(): RenderTotals {
  return { ...totals, byPanel: { ...totals.byPanel }, byName: { ...totals.byName }, byWhere: { ...totals.byWhere } };
}

function panelOf(fiber: Fiber, outer: string): string {
  if (fiber.tag !== HOST_COMPONENT) return outer;
  const className = (fiber.memoizedProps as { className?: unknown } | undefined)?.className;
  if (typeof className !== 'string') return outer;
  const classes = className.split(' ');
  return PANELS.find(([c]) => classes.includes(c))?.[1] ?? outer;
}

function nameOf(fiber: Fiber): string {
  const type = fiber.type as { displayName?: string; name?: string; render?: { name?: string } } | null;
  return type?.displayName || type?.name || type?.render?.name || 'anonymous';
}

const listeners = new Set<() => void>();

/** Hears each commit, after it is counted. */
export function onCommit(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/**
 * Counts what rendered in the commit whose root fiber is `root`: only subtrees whose flags say something in them
 * rendered are walked, so a commit costs the size of what changed.
 */
export function tallyCommit(root: Fiber) {
  totals.commits += 1;
  const stack: [Fiber, string][] = [];
  if (root.child && root.subtreeFlags & PERFORMED_WORK) stack.push([root.child, 'other']);
  while (stack.length > 0) {
    const [first, outer] = stack.pop()!;
    for (let f: Fiber | null = first; f; f = f.sibling) {
      if (COMPONENT_TAGS.has(f.tag) && f.flags & PERFORMED_WORK) {
        totals.renders += 1;
        totals.byPanel[outer] = (totals.byPanel[outer] ?? 0) + 1;
        const name = nameOf(f);
        totals.byName[name] = (totals.byName[name] ?? 0) + 1;
        const where = `${outer} ${name}`;
        totals.byWhere[where] = (totals.byWhere[where] ?? 0) + 1;
      }
      if (f.child && f.subtreeFlags & PERFORMED_WORK) stack.push([f.child, panelOf(f, outer)]);
    }
  }
  for (const listener of listeners) listener();
}

type Hook = {
  supportsFiber?: boolean;
  inject?: (internals: unknown) => number;
  onCommitFiberRoot?: (id: number, root: { current: Fiber }, ...rest: unknown[]) => void;
  [key: string]: unknown;
};

/**
 * Counts every commit from now on, through React's DevTools hook: a hook already there (development's fast refresh)
 * keeps working, with the count added to it.
 */
export function installRenderHook() {
  const w = window as unknown as { __REACT_DEVTOOLS_GLOBAL_HOOK__?: Hook };
  const hook: Hook = (w.__REACT_DEVTOOLS_GLOBAL_HOOK__ ??= {
    supportsFiber: true,
    renderers: new Map(),
    inject: () => 1,
    onCommitFiberUnmount: () => {},
    onPostCommitFiberRoot: () => {},
    checkDCE: () => {},
  });
  const before = hook.onCommitFiberRoot;
  hook.onCommitFiberRoot = (id, root, ...rest) => {
    before?.call(hook, id, root, ...rest);
    tallyCommit(root.current);
  };
}

if (pagePerfOn()) installRenderHook();
