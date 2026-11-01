import { call } from './call';

/**
 * The eyedropper seam: with `klipolo_platform::screen_color` in Rust, the only code that knows the system's eyedropper
 * exists. macOS has one (`NSColorSampler`): the system draws the loupe and hands back only the colour clicked, so it
 * needs no Screen Recording permission. Windows and Linux have none yet, so no eyedropper shows there. It starts only
 * on the person's press; tests mock it and never call it.
 */
export const screenColor = {
  /** This computer has an eyedropper (a Mac). */
  supported: (): boolean => /^Mac/.test(navigator.platform),
  /** Shows the eyedropper: the colour the person clicked, as sRGB `#RRGGBB`, or null when they cancelled. */
  pick: (): Promise<string | null> => call<string | null>('screen_color_pick'),
};
