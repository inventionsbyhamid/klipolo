import { describe, expect, it } from 'vitest';
import { fixtureProject } from '../../../test/fixtures';
import { deleteOps, nudgeOps, trimToPlayheadOps } from './keyOps';

describe('keyboard edits', () => {
  const p = fixtureProject();
  it("Q trims the selected clip's start to the playhead (the model ripples)", () => {
    expect(trimToPlayheadOps(p, 'b', 22_000_000, 'start', 30)).toEqual([
      { type: 'trimClip', id: 'b', edge: 'start', sourceUs: 2_000_000 },
    ]);
  });
  it("W trims a layer's end to the playhead, nothing when the playhead is outside it", () => {
    expect(trimToPlayheadOps(p, 'title', 2_000_000, 'end', 30)).toEqual([
      { type: 'trimItem', id: 'title', edge: 'end', atUs: 2_000_000 },
    ]);
    expect(trimToPlayheadOps(p, 'title', 10_000_000, 'end', 30)).toEqual([]);
  });
  it('nudges a layer by frames on the grid and a clip by places', () => {
    expect(nudgeOps(p, 'title', 3, 30)).toEqual([
      { type: 'moveItem', id: 'title', trackId: null, startUs: 1_100_000 },
    ]);
    expect(nudgeOps(p, 'b', -1, 30)).toEqual([{ type: 'moveClip', id: 'b', index: 0 }]);
    expect(nudgeOps(p, 'a', -1, 30)).toEqual([]);
  });
  it('⌫ takes a song laid in pieces whole, a sound or a layer alone, and nothing on a locked track', () => {
    const bed = fixtureProject();
    const audio = bed.timeline.tracks[1];
    const [song] = audio.items;
    const music = song.content.type === 'audio' ? { ...song.content, duck: -12 } : song.content;
    audio.items = ['bed', 'bed-2', 'bed-3'].map((id, k) => ({
      ...song,
      id,
      startUs: k * 10_000_000,
      durationUs: 10_000_000,
      content: music,
    }));
    const removed = (id: string) => deleteOps(bed, id).map((op) => (op.type === 'removeItem' ? op.id : op.type));
    expect(removed('bed-3')).toEqual(['bed', 'bed-2', 'bed-3']);
    expect(removed('title')).toEqual(['title']);
    // The fixture's song is a sound effect (no ducking): it goes alone.
    expect(deleteOps(p, 'song')).toEqual([{ type: 'removeItem', id: 'song' }]);
    audio.locked = true;
    expect(deleteOps(bed, 'bed')).toEqual([]);
  });
});
