import type { Theme } from '../bindings/Theme';
import { call } from './call';

/**
 * Settings' Appearance on the window: Rust saves the theme and paints the window's own colour, behind the see-through
 * page, with the theme's canvas, now and at the next launch before the page loads.
 */
export function setAppearance(theme: Theme): Promise<void> {
  return call<void>('appearance_set', { theme });
}
