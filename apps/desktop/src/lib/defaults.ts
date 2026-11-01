import type { Asset } from '../bindings/Asset';
import type { BoxN } from '../bindings/BoxN';
import type { CameraLayout } from '../bindings/CameraLayout';
import type { Clip } from '../bindings/Clip';
import type { FormatKind } from '../bindings/FormatKind';
import type { Item } from '../bindings/Item';
import type { Look } from '../bindings/Look';
import type { MotionUse } from '../bindings/MotionUse';
import type { Recording } from '../bindings/Recording';
import { builtinId } from './contrib';
import { TITLE_SIZE, titleLayout } from './layout';
import { recordingDurationUs } from './recording';

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

/** Where the camera bubble sits in each main format, as `CameraLayout::for_primary` puts it. */
const BUBBLE_RECT: Record<FormatKind, BoxN> = {
  landscape: { x: 0.76, y: 0.6044, w: 0.2, h: 0.3556 },
  portrait: { x: 0.6044, y: 0.76, w: 0.3556, h: 0.2 },
  square: { x: 0.6044, y: 0.6044, w: 0.3556, h: 0.3556 },
};

/**
 * The camera a recording's clip starts with in a project whose main format is `primary`, as `CameraLayout::for_primary`
 * makes it: a round, bordered bubble at the bottom right (split at the bottom in 9:16), mirrored.
 */
export function cameraLayoutFor(primary: FormatKind): CameraLayout {
  return {
    mode: 'bubble',
    modeOverrides: { portrait: 'splitBottom' },
    shape: 'circle',
    border: true,
    mirror: true,
    bubble: { anchor: 'bottomRight', rect: BUBBLE_RECT[primary], rotation: 0, opacity: 1, radius: 0, overrides: {} },
  };
}

/** The whole of `recording` as a main-track clip (`klipolo_capture::clip_for`), with `camera` for its camera. */
export function newRecordingClip(recording: Recording, camera: CameraLayout | null): Clip {
  return {
    id: newId(),
    source: { type: 'recording', id: recording.id },
    sourceInUs: 0,
    sourceOutUs: recordingDurationUs(recording),
    speed: 1,
    volume: 1,
    muted: false,
    fadeInUs: 0,
    fadeOutUs: 0,
    look: DEFAULT_LOOK,
    camera,
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
