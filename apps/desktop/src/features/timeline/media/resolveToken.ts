/** A design token as a colour canvas can use: CSS variables hold `light-dark(…)`, which only an element resolves. */
const cache = new Map<string, string>();
let probe: HTMLElement | null = null;
const listeners = new Set<() => void>();
/** Bumped on every theme or pixel-density change, so what draws on a canvas can tell it must redraw. */
let version = 0;

export function resolveToken(name: string): string {
  const hit = cache.get(name);
  if (hit !== undefined) return hit;
  probe ??= Object.assign(document.createElement('span'), { hidden: true });
  if (!probe.isConnected) document.body.append(probe);
  probe.style.color = `var(${name})`;
  const value = getComputedStyle(probe).color;
  cache.set(name, value);
  return value;
}

function notify() {
  version++;
  for (const listener of listeners) listener();
}

/** The theme changed (system or the app's setting): drop resolved colours and redraw. */
export function themeChanged() {
  cache.clear();
  notify();
}

/** The screen's pixel density, watched while anything listens: a window moved to another display redraws sharp. */
let density: MediaQueryList | null = null;

function watchDensity() {
  density?.removeEventListener('change', densityChanged);
  density = window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
  density.addEventListener('change', densityChanged);
}

function densityChanged() {
  watchDensity();
  notify();
}

/** Calls `listener` when what canvases draw must be redrawn: the theme or the screen's pixel density changed. */
export function onThemeChange(listener: () => void): () => void {
  if (listeners.size === 0) watchDensity();
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    if (listeners.size > 0) return;
    density?.removeEventListener('change', densityChanged);
    density = null;
  };
}

export function themeVersion(): number {
  return version;
}
