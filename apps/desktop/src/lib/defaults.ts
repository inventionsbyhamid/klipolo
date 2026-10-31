import type { Asset } from '../bindings/Asset';
import type { Clip } from '../bindings/Clip';
import type { FormatKind } from '../bindings/FormatKind';
import type { Item } from '../bindings/Item';
import type { Look } from '../bindings/Look';
import type { MotionUse } from '../bindings/MotionUse';
import { builtinId } from './contrib';
import { TITLE_SIZE, titleLayout } from './layout';

export function newId(): string {
  return crypto.randomUUID();
}

export const DEFAULT_LOOK: Look = { preset: null, strength: 1, lutId: null, lutStrength: 1 };

export function newClipFor(asset: Asset): Clip {
  return {
    id: newId(),
    source: { type: 'asset', id: asset.id },
    sourceInUs: 0,
    sourceOutUs: asset.durationUs,
    speed: 1,
    volume: 1,
    muted: false,
    fadeInUs: 0,
    fadeOutUs: 0,
    look: DEFAULT_LOOK,
    camera: null,
    framing: {},
    effects: [],
  };
}

/** A built-in motion preset by its word, 0.3 s long (at most `durationUs`), with its default settings. */
export function preset(word: string, durationUs = 300_000): MotionUse {
  return { preset: builtinId(`motion.${word}`), params: {}, durationUs: Math.min(300_000, durationUs) };
}

/**
 * A new title, as Rust's Title preset makes one (`media::text`): Inter 700, white with a shadow, centred at the top
 * of the frame in `primary` and in its own place in each other format (in 9:16 inside what the apps leave clear), fading
 * in and out over 0.3 s.
 */
export function newTextItem(text: string, startUs: number, durationUs: number, primary: FormatKind): Item {
  return {
    id: newId(),
    startUs,
    durationUs,
    attach: null,
    layout: {
      ...titleLayout(primary),
      motion: { enter: preset('fade', durationUs), exit: preset('fade', durationUs) },
    },
    content: {
      type: 'text',
      text,
      style: {
        font: { family: 'Inter', source: { type: 'bundled' } },
        size: TITLE_SIZE,
        weight: 700,
        italic: false,
        color: '#FFFFFF',
        background: null,
        outline: null,
        shadow: true,
        align: 'center',
      },
    },
    effects: [],
  };
}
