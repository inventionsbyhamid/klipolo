import { invoke } from '@tauri-apps/api/core';
import type { BundledFont } from '../bindings/BundledFont';
import { call } from './call';

/** Klipolo's bundled fonts (`crates/klipolo-app/src/commands/fonts.rs`): the 12 families a text can use. */
export const fonts = {
  /** Every face of the families the Font menu offers, in its order. */
  bundled: () => call<BundledFont[]>('fonts_bundled'),
  /** The bytes of `family`'s face nearest `weight` and `italic`: the very file the engine draws the text with. */
  face: (family: string, weight: number, italic: boolean) =>
    invoke<ArrayBuffer>('fonts_face', { family, weight, italic }),
};
