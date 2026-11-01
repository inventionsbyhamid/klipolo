/**
 * jsdom has no ResizeObserver; the setup's shim never calls back. `captureResizeObservers` swaps in one that records
 * each observer's callback and targets, so a test can say when a size changed (with `stubSize`, `./layoutBox`). Call
 * it before rendering: observers made earlier aren't seen. Shared by the page's tests, so a change to it must keep
 * every one of them green.
 *
 * Unlike a real ResizeObserver:
 * - `observe()` sends no first callback: call `fire()` after rendering;
 * - `fire()` calls every live observer, whether or not its targets changed;
 * - nested captures must be restored newest first, or a recording one stays installed.
 */
export function captureResizeObservers(): { fire(): void; restore(): void } {
  const original = Object.getOwnPropertyDescriptor(window, 'ResizeObserver');
  const live = new Set<Recording>();

  class Recording implements ResizeObserver {
    readonly targets = new Set<Element>();
    constructor(readonly callback: ResizeObserverCallback) {}
    observe(target: Element) {
      this.targets.add(target);
      live.add(this);
    }
    unobserve(target: Element) {
      this.targets.delete(target);
    }
    disconnect() {
      this.targets.clear();
      live.delete(this);
    }
  }

  Object.defineProperty(window, 'ResizeObserver', { configurable: true, writable: true, value: Recording });
  return {
    /** Calls every observer that watches something, once, with an entry per target (its rect now). */
    fire() {
      for (const observer of [...live]) {
        const entries = [...observer.targets].map((target) => {
          const r = target.getBoundingClientRect();
          const size = { inlineSize: r.width, blockSize: r.height };
          return { target, contentRect: r, borderBoxSize: [size], contentBoxSize: [size] };
        });
        if (entries.length > 0) observer.callback(entries as unknown as ResizeObserverEntry[], observer);
      }
    },
    /** Puts the setup's shim back. */
    restore() {
      if (original) Object.defineProperty(window, 'ResizeObserver', original);
      else delete (window as { ResizeObserver?: unknown }).ResizeObserver;
    },
  };
}
