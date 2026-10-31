import { describe, expect, it } from 'vitest';
import type { Asset } from '../bindings/Asset';
import { fixtureProject } from '../test/fixtures';
import { newClipFor, newTextItem } from './defaults';
import { resolveRect } from './layout';
import { imageRect, placementOps } from './placement';
import { formatDuration, formatPrecise, formatTime, snapToFrame } from './time';
import {
  clipAt,
  clipStarts,
  findItem,
  itemLabel,
  sourceLabel,
  mainDurationUs,
  videoDurationUs,
  visualItemsAt,
} from './timeline';

const S = 1_000_000;

describe('time', () => {
  it('formats durations', () => {
    expect(formatTime(0)).toBe('0:00');
    expect(formatTime(65 * S)).toBe('1:05');
    expect(formatTime(3725 * S)).toBe('1:02:05');
    expect(formatTime(-5)).toBe('0:00');
    expect(formatPrecise(1_250_000)).toBe('0:01.2');
    expect([formatTime(59_600_000), formatDuration(59_600_000)]).toEqual(['0:59', '1:00']);
  });
  it('snaps to frames', () => {
    expect(snapToFrame(1_016_000, 30)).toBe(1_000_000);
    expect(snapToFrame(1_020_000, 30)).toBe(1_033_333);
  });
});

describe('timeline queries', () => {
  const p = fixtureProject();
  it('computes starts, duration and the clip at a time', () => {
    expect(clipStarts(p.timeline.main)).toEqual([0, 20 * S]);
    expect(mainDurationUs(p.timeline.main)).toBe(30 * S);
    expect(clipAt(p.timeline.main, 25 * S)?.clip.id).toBe('b');
    expect(clipAt(p.timeline.main, 30 * S)).toBeNull();
  });
  it('lasts until the main track or its last picture layer ends', () => {
    // 30 s of clips; a 30 s song that a longer one doesn't stretch the video by.
    const p = fixtureProject();
    expect(videoDurationUs(p.timeline)).toBe(30 * S);
    const [titles, sound] = p.timeline.tracks;
    const longSong = [{ ...sound.items[0], durationUs: 90 * S }];
    const song = { ...p.timeline, tracks: [titles, { ...sound, items: longSong }] };
    expect(videoDurationUs(song)).toBe(30 * S);
    // A title to 40 s makes it 40 s, on a hidden track too; layers alone make a video.
    const late = [{ ...titles.items[0], startUs: 35 * S, durationUs: 5 * S }];
    const longer = { ...p.timeline, tracks: [{ ...titles, hidden: true, items: late }, sound] };
    expect(videoDurationUs(longer)).toBe(40 * S);
    expect(videoDurationUs({ ...longer, main: [] })).toBe(40 * S);
    expect(videoDurationUs({ main: [], tracks: [] })).toBe(0);
    // An adjustment box only changes what is below it: it doesn't make the video longer.
    const box = [
      { ...titles.items[0], startUs: 35 * S, durationUs: 5 * S, content: { type: 'adjustment' as const } },
    ];
    expect(videoDurationUs({ ...p.timeline, tracks: [{ ...titles, items: box }, sound] })).toBe(30 * S);
  });
  it('finds visible overlay items at a time', () => {
    expect(visualItemsAt(p, 2 * S).map((i) => i.id)).toEqual(['title']);
    expect(visualItemsAt(p, 5 * S)).toEqual([]);
  });
  it('labels sources and items', () => {
    expect(sourceLabel(p, p.timeline.main[0].source)).toBe('screen.mp4');
    expect(itemLabel(p, findItem(p, 'title')!.item)).toBe('Set up billing');
    expect(itemLabel(p, findItem(p, 'song')!.item)).toBe('music.mp3');
  });
  it('names a missing motion graphic as one', () => {
    const title = findItem(p, 'title')!.item;
    const generator = (source: string) => ({
      ...title,
      content: { type: 'generator' as const, source, params: {} },
    });
    expect(itemLabel(p, generator('com.acme.fx/scene.lower-third'))).toBe('Missing motion graphic');
    expect(itemLabel(p, generator('com.acme.fx/generator.waves'))).toBe('Missing generator');
  });
});

describe('layout', () => {
  it('matches the Rust resolve_rect vectors', () => {
    const title = {
      anchor: 'topLeft' as const,
      rect: { x: 0.05, y: 0.05, w: 0.4, h: 0.1 },
      rotation: 0,
      opacity: 1,
      radius: 0,
      overrides: {},
    };
    const r = resolveRect(title, 'landscape', 'portrait');
    expect(r.x).toBeCloseTo(0.088889, 4);
    expect(r.y).toBeCloseTo(0.028125, 4);
    expect(r.w).toBeCloseTo(0.711111, 4);
    expect(r.h).toBeCloseTo(0.05625, 4);
    const caption = { ...title, anchor: 'bottomCenter' as const, rect: { x: 0.1, y: 0.78, w: 0.8, h: 0.12 } };
    const c = resolveRect(caption, 'landscape', 'portrait');
    expect([c.x, c.y, c.w, c.h].map((v) => Number(v.toFixed(4)))).toEqual([0, 0.8763, 1, 0.0675]);
  });
});

describe('placement', () => {
  const video: Asset = {
    id: 'v',
    kind: 'video',
    path: '/v.mp4',
    name: 'v.mp4',
    durationUs: 8 * S,
    width: 1920,
    height: 1080,
    fps: 0,
    hasAudio: true,
  };
  const song: Asset = {
    id: 's',
    kind: 'audio',
    path: '/s.mp3',
    name: 's.mp3',
    durationUs: 90 * S,
    width: 0,
    height: 0,
    fps: 0,
    hasAudio: true,
  };
  const logo: Asset = {
    id: 'l',
    kind: 'image',
    path: '/l.png',
    name: 'l.png',
    durationUs: 0,
    width: 400,
    height: 200,
    fps: 0,
    hasAudio: false,
  };
  it('audio_and_images_go_to_the_library_not_the_main_track', () => {
    const ops = placementOps([video, song, logo], fixtureProject(), 2 * S);
    expect(ops).toHaveLength(2);
    expect(ops[0]).toMatchObject({ type: 'addAssets', assets: [video, song, logo] });
    expect(ops[1]).toMatchObject({ type: 'insertClip', index: null, clip: { sourceOutUs: 8 * S } });
    // A sound and a picture alone: into the project's files, nothing on the timeline.
    expect(placementOps([song, logo], fixtureProject(), 0)).toEqual([
      { type: 'addAssets', assets: [song, logo] },
    ]);
  });
  it('keeps a tall picture whole where it is placed', () => {
    const phone: Asset = { ...logo, id: 'p', name: 'p.png', width: 1170, height: 2532 };
    const rect = imageRect(phone, 1920, 1080);
    expect(rect.h).toBe(1);
    expect(rect.w).toBeCloseTo((1080 * 1170) / (2532 * 1920));
  });
  it('builds valid defaults', () => {
    expect(newClipFor(video)).toMatchObject({
      source: { type: 'asset', id: 'v' },
      sourceInUs: 0,
      sourceOutUs: 8 * S,
      speed: 1,
    });
    const title = newTextItem('Hi', 0, 3 * S, 'portrait');
    expect(title.content).toMatchObject({ type: 'text', text: 'Hi', style: { size: 96 } });
    // Placed in every format at once: its rect in the primary, its own place in each other one.
    expect(Object.keys(title.layout.overrides).sort()).toEqual(['landscape', 'square']);
    expect(title.layout.rect).not.toBe(title.layout.overrides.landscape);
  });
});
