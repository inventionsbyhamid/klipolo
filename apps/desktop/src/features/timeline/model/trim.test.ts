import { describe, expect, it } from 'vitest';
import { fixtureProject } from '../../../test/fixtures';
import { trimClip, trimItemEdge } from './trim';

const p = fixtureProject();
const [a, b] = p.timeline.main;

describe('trims', () => {
  it("a clip's end stops at the end of its media", () => {
    expect(trimClip(p, a, 'end', 5_000_000, 30)).toEqual({ sourceUs: 25_000_000, deltaUs: 5_000_000 });
    expect(trimClip(p, b, 'end', 5_000_000, 30)).toEqual({ sourceUs: 10_000_000, deltaUs: 0 });
  });

  it("a clip's start can't go before its file's start, and a trim keeps one frame", () => {
    expect(trimClip(p, a, 'start', -1_000_000, 30)).toEqual({ sourceUs: 0, deltaUs: 0 });
    expect(trimClip(p, b, 'end', -20_000_000, 30)).toEqual({ sourceUs: 33_333, deltaUs: -9_966_667 });
  });

  it('speed scales the source a trim moves', () => {
    const fast = { ...b, speed: 2 };
    expect(trimClip(p, fast, 'end', -1_000_000, 30).sourceUs).toBe(8_000_000);
  });

  it("a layer's edge stops at its neighbour on the track", () => {
    const track = { ...p.timeline.tracks[0] };
    const title = track.items[0];
    track.items = [title, { ...title, id: 'next', startUs: 6_000_000 }];
    expect(trimItemEdge(p, track, title, 'end', 100_000_000, 30)).toBe(6_000_000);
    expect(trimItemEdge(p, track, title, 'start', -5_000_000, 30)).toBe(0);
  });

  it("a start trim at 2× never rounds past its file's start", () => {
    // 33,333 µs of footage is 16,666.5 µs of timeline at 2×: the edge stops at 16,666, which is footage 1 µs in.
    const fast = { ...b, speed: 2, sourceInUs: 33_333 };
    expect(trimClip(p, fast, 'start', -1_000_000, 30)).toEqual({ sourceUs: 1, deltaUs: -16_666 });
    const track = { ...p.timeline.tracks[0] };
    const video = {
      ...track.items[0],
      startUs: 5_000_000,
      content: {
        type: 'video' as const,
        assetId: 'broll',
        sourceInUs: 33_333,
        speed: 2,
        volume: 1,
        muted: false,
      },
    };
    track.items = [video];
    expect(trimItemEdge(p, track, video, 'start', 0, 30)).toBe(5_000_000 - 16_666);
  });
});
