import { act, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../bridge/appearance', () => ({ setAppearance: vi.fn(() => Promise.resolve()) }));
const { setAppearance } = await import('../bridge/appearance');
const { useUi } = await import('../state/ui');
const { onThemeChange } = await import('../lib/resolveToken');
const { resolveTheme, useThemeSync } = await import('./useThemeSync');

const STORED = 'klipolo.theme';

function Synced() {
  useThemeSync();
  return null;
}

/** The system's appearance, as the page asks for it: `prefers-color-scheme`, which the test turns dark or light. */
function fakeAppearance() {
  const listeners = new Set<() => void>();
  let dark = false;
  const original = window.matchMedia;
  window.matchMedia = (query: string) =>
    ({
      get matches() {
        return query.includes('prefers-color-scheme: dark') && dark;
      },
      media: query,
      onchange: null,
      addEventListener: (_: string, listener: () => void) => {
        if (query.includes('prefers-color-scheme')) listeners.add(listener);
      },
      removeEventListener: (_: string, listener: () => void) => listeners.delete(listener),
      addListener: () => undefined,
      removeListener: () => undefined,
      dispatchEvent: () => false,
    }) as unknown as MediaQueryList;
  return {
    turn(next: 'dark' | 'light') {
      dark = next === 'dark';
      for (const listener of [...listeners]) listener();
    },
    listening: () => listeners.size,
    restore: () => {
      window.matchMedia = original;
    },
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.removeItem(STORED);
  act(() => useUi.getState().setTheme('system'));
  vi.clearAllMocks();
});

afterEach(() => {
  delete document.documentElement.dataset.theme;
});

describe('the theme', () => {
  it('is the system’s until one is picked, and the pick is kept on this Mac', async () => {
    const fresh = async () => {
      vi.resetModules();
      return (await import('../state/ui')).useUi.getState().theme;
    };
    localStorage.removeItem(STORED);
    expect(await fresh()).toBe('system');
    useUi.getState().setTheme('light');
    expect(localStorage.getItem(STORED)).toBe('light');
    expect(await fresh()).toBe('light');
    useUi.getState().setTheme('dark');
    expect(await fresh()).toBe('dark');
    // A value this Klipolo doesn't know reads as the system's.
    localStorage.setItem(STORED, 'sepia');
    expect(await fresh()).toBe('system');
  });

  it('System follows the system’s appearance; Light and Dark win over it', () => {
    expect(resolveTheme('system', true)).toBe('dark');
    expect(resolveTheme('system', false)).toBe('light');
    expect(resolveTheme('light', true)).toBe('light');
    expect(resolveTheme('dark', false)).toBe('dark');
  });

  it('data-theme forces Light or Dark on the page, and System leaves it to the system', () => {
    const root = document.documentElement;
    render(<Synced />);
    expect(root).not.toHaveAttribute('data-theme');
    act(() => useUi.getState().setTheme('dark'));
    expect(root).toHaveAttribute('data-theme', 'dark');
    act(() => useUi.getState().setTheme('light'));
    expect(root).toHaveAttribute('data-theme', 'light');
    act(() => useUi.getState().setTheme('system'));
    expect(root).not.toHaveAttribute('data-theme');
  });

  it('a theme kept from before is on the page from its first paint', () => {
    act(() => useUi.getState().setTheme('dark'));
    render(<Synced />);
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
  });

  it('tells the window each theme, so its own colour follows', () => {
    render(<Synced />);
    expect(setAppearance).toHaveBeenLastCalledWith('system');
    act(() => useUi.getState().setTheme('light'));
    expect(setAppearance).toHaveBeenLastCalledWith('light');
    act(() => useUi.getState().setTheme('dark'));
    expect(setAppearance).toHaveBeenLastCalledWith('dark');
  });

  it('canvases draw again in the new colours when the theme or the system’s appearance changes', () => {
    const appearance = fakeAppearance();
    const redraw = vi.fn();
    const stop = onThemeChange(redraw);
    try {
      const { unmount } = render(<Synced />);
      redraw.mockClear();
      act(() => useUi.getState().setTheme('dark'));
      expect(redraw).toHaveBeenCalled();
      // Following the system: its change redraws, with no setting changed.
      act(() => useUi.getState().setTheme('system'));
      redraw.mockClear();
      act(() => appearance.turn('dark'));
      expect(redraw).toHaveBeenCalledTimes(1);
      unmount();
      expect(appearance.listening()).toBe(0);
    } finally {
      stop();
      appearance.restore();
    }
  });
});
