import { useEffect, useLayoutEffect } from 'react';
import type { Theme } from '../bindings/Theme';
import { setAppearance } from '../bridge/appearance';
import { themeChanged } from '../lib/resolveToken';
import { useUi } from '../state/ui';

/** The theme the page shows: the system's appearance for System, else the one picked. */
export function resolveTheme(theme: Theme, systemDark: boolean): 'light' | 'dark' {
  if (theme === 'system') return systemDark ? 'dark' : 'light';
  return theme;
}

/**
 * The tokens follow the system's appearance on their own (CSS `light-dark()`), so `data-theme` is set only while Light
 * or Dark is picked, and taken off again for System. A layout effect sets it before the first paint, so a theme picked
 * before never shows the system's for a frame. Rust paints the window's own colour, behind the see-through page, to
 * match (`setAppearance`).
 *
 * Canvases can't use the tokens as they are: the timeline's waveforms resolve them to colours (`resolveToken`). So a
 * theme applied here, and a change of the system's appearance, tell them to draw again (`themeChanged`).
 */
export function useThemeSync() {
  const theme = useUi((s) => s.theme);
  useLayoutEffect(() => {
    const root = document.documentElement;
    if (theme === 'system') delete root.dataset.theme;
    else root.dataset.theme = theme;
    themeChanged();
    void setAppearance(theme).catch(() => undefined);
  }, [theme]);
  useEffect(() => {
    const scheme = window.matchMedia('(prefers-color-scheme: dark)');
    scheme.addEventListener('change', themeChanged);
    return () => scheme.removeEventListener('change', themeChanged);
  }, []);
}
