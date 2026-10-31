import { describe, expect, it } from 'vitest';
import type { Item } from '../bindings/Item';
import { fixtureProject, titleItem } from '../test/fixtures';
import { hasOwnSound } from './timeline';

const p = fixtureProject();
const layer = (content: Item['content']): Item => ({ ...titleItem, content });
const video = (assetId: string) =>
  layer({ type: 'video', assetId, sourceInUs: 0, speed: 1, volume: 1, muted: false });

describe('a layer’s own sound (what its Mute silences)', () => {
  it('is a sound layer’s, and a video layer’s whose file has sound; nothing else has one', () => {
    expect(hasOwnSound(p, p.timeline.tracks[1].items[0])).toBe(true);
    expect(hasOwnSound(p, video('broll'))).toBe(true);
    // The recording's camera file has no sound of its own.
    expect(hasOwnSound(p, video('camera'))).toBe(false);
    expect(hasOwnSound(p, video('gone'))).toBe(false);
    expect(hasOwnSound(p, titleItem)).toBe(false);
    expect(hasOwnSound(p, layer({ type: 'image', assetId: 'logo' }))).toBe(false);
  });
});
