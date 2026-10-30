import type { Asset } from '../bindings/Asset';
import type { Clip } from '../bindings/Clip';
import type { Item } from '../bindings/Item';
import type { Look } from '../bindings/Look';
import type { MotionUse } from '../bindings/MotionUse';
import { builtinId } from './contrib';

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

/** A new title: it fades in and out over 0.3 s. */
export function newTextItem(text: string, startUs: number, durationUs: number): Item {
  return {
    id: newId(),
    startUs,
    durationUs,
    attach: null,
    layout: {
      anchor: 'topCenter',
      rect: { x: 0.2, y: 0.08, w: 0.6, h: 0.12 },
      rotation: 0,
      opacity: 1,
      radius: 0,
      overrides: {},
      motion: { enter: preset('fade', durationUs), exit: preset('fade', durationUs) },
    },
    content: {
      type: 'text',
      text,
      style: {
        font: { family: 'Inter', source: { type: 'bundled' } },
        size: 72,
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
