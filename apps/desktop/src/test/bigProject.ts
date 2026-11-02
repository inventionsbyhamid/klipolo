import type { Asset } from '../bindings/Asset';
import type { Clip } from '../bindings/Clip';
import type { Item } from '../bindings/Item';
import type { ItemContent } from '../bindings/ItemContent';
import type { Key } from '../bindings/Key';
import type { Layout } from '../bindings/Layout';
import type { Marker } from '../bindings/Marker';
import type { Motion } from '../bindings/Motion';
import type { PageView } from '../bindings/PageView';
import type { Project } from '../bindings/Project';
import type { Track } from '../bindings/Track';
import type { WordView } from '../bindings/WordView';
import { DEFAULT_LOOK, preset } from '../lib/defaults';
import { CURSOR_STYLE } from './fixtures';

/**
 * Big projects at the scale people make them, for the page's performance tests and to open in the app: a 2-hour talk
 * cut into 300 pieces with Word pop captions, 200 layers (titles, stickers, sound effects, music with volume keys,
 * zooms, motion keys) and 4 video layers, and a 60 s short with what one prompt adds. Their media are the files the
 * engine's own perf media names (`talk-1080p-2h.mp4`, four 60 s clips) in `mediaDir`, and the library pack's music,
 * sounds and stickers, which a project finds wherever the app keeps its pack.
 */

const S = 1_000_000;
/** The talk's length: two hours. */
export const TWO_HOURS_US = 7200 * S;
const WORD_POP = 'klipolo.builtin/caption.word-pop';
const PACK = 'klipolo-pack-';

function video(id: string, file: string, mediaDir: string, seconds: number, w: number, h: number, fps: number) {
  const extra = id === 'clip-d' ? { rotation: 90 } : {};
  return {
    id,
    kind: 'video' as const,
    path: `${mediaDir}/${file}`,
    name: file,
    durationUs: seconds * S,
    width: w,
    height: h,
    fps,
    hasAudio: true,
    ...extra,
  } satisfies Asset;
}

/** A library pack file: the app points it at its own pack's copy as the project opens. */
function pack(item: string, folder: 'music' | 'sounds' | 'stickers', seconds = 0): Asset {
  const kind = folder === 'stickers' ? 'image' : 'audio';
  return {
    id: `${PACK}${item}`,
    kind,
    path: `/Applications/Klipolo.app/Contents/Resources/library/${folder}/${item}.${kind === 'image' ? 'png' : 'wav'}`,
    name: item,
    durationUs: Math.round(seconds * S),
    width: kind === 'image' ? 1024 : 0,
    height: kind === 'image' ? 1024 : 0,
    fps: 0,
    hasAudio: kind === 'audio',
  };
}

const MUSIC = ['soft-ambient', 'upbeat-loop', 'chill-beat'];
const SOUNDS = ['whoosh', 'swoosh', 'pop'];
const STICKERS = ['heart', 'star', 'arrow'];

function assets(mediaDir: string): Asset[] {
  return [
    video('talk', 'talk-1080p-2h.mp4', mediaDir, 7200, 1920, 1080, 30),
    video('clip-a', 'clip-a-1080p30.mp4', mediaDir, 60, 1920, 1080, 30),
    video('clip-b', 'clip-b-2160p30.mp4', mediaDir, 60, 3840, 2160, 30),
    video('clip-c', 'clip-c-1080p60.mp4', mediaDir, 60, 1920, 1080, 60),
    video('clip-d', 'clip-d-phone-hevc.mov', mediaDir, 60, 1920, 1080, 30),
    ...MUSIC.map((m) => pack(m, 'music', 16)),
    ...[0.9, 0.5, 0.16].map((len, i) => pack(SOUNDS[i], 'sounds', len)),
    ...STICKERS.map((s) => pack(s, 'stickers')),
  ];
}

/** Each sound effect's length: the file's, or half a second. */
const SOUND_LEN = [0.5, 0.5, 0.16];

const keys = (pairs: [number, number][]): Key[] => pairs.map(([t, v]) => ({ tUs: Math.round(t * S), v }));

function layout(rect: Layout['rect'], motion?: Motion): Layout {
  return { anchor: 'center', rect, rotation: 0, opacity: 1, radius: 0, overrides: {}, ...(motion ? { motion } : {}) };
}

/** Motion keys over a layer `len` seconds long: a drift right and a swell, with a turn when `turn`. */
function moving(len: number, turn = false): Motion {
  return {
    enter: preset('pop'),
    exit: preset('fade'),
    keys: {
      x: keys([[0, -0.05], [len, 0.05]]),
      y: [],
      scale: keys([[0, 0.9], [len / 2, 1.1], [len, 1]]),
      rotation: turn ? keys([[0, -8], [len, 8]]) : [],
      opacity: [],
    },
  };
}

function layer(id: string, start: number, len: number, content: ItemContent, lay: Layout): Item {
  return { id, startUs: Math.round(start * S), durationUs: Math.round(len * S), attach: null, layout: lay, content, effects: [] };
}

function title(id: string, start: number, len: number, text: string, keyed: boolean): Item {
  const motion: Motion = keyed ? moving(len) : { enter: preset('fade'), exit: preset('fade') };
  return layer(
    id,
    start,
    len,
    {
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
    layout({ x: 0.2, y: 0.08, w: 0.6, h: 0.12 }, motion),
  );
}

const sound = (id: string, start: number, len: number, assetId: string, extra: Partial<ItemContent> = {}): Item =>
  layer(
    id,
    start,
    len,
    { type: 'audio', assetId, sourceInUs: 0, volume: 1, fadeInUs: 0, fadeOutUs: 0, effect: null, ...extra } as ItemContent,
    layout({ x: 0, y: 0, w: 1, h: 1 }),
  );

/**
 * One song of `pieces` back to back from `start` s, each the file's 16 s (the last `lastLen`), ducked under the voice,
 * fading in and out at its ends, with volume keys on every piece.
 */
function songAt(prefix: string, start: number, pieces: number, file: string, lastLen = 16): Item[] {
  return Array.from({ length: pieces }, (_, i) => {
    const len = i === pieces - 1 ? lastLen : 16;
    return sound(`${prefix}-${i}`, start + i * 16, len, `${PACK}${file}`, {
      volume: 0.4,
      duck: -12,
      fadeInUs: i === 0 ? S : 0,
      fadeOutUs: i === pieces - 1 ? 2 * S : 0,
      volumeKeys: keys([[0, 0.3], [len / 4, 0.5], [(len * 3) / 4, 0.5], [len, 0.3]]),
    });
  });
}

function track(id: string, kind: Track['kind'], name: string, items: Item[]): Track {
  return { id: `track-${id}`, kind, name, hidden: false, muted: false, locked: false, items };
}

/** The main track's pieces: `count` cuts of the talk back to back from `fromUs`, each `pieceUs` long. */
function pieces(prefix: string, count: number, pieceUs: number, gapUs = 0): Clip[] {
  return Array.from({ length: count }, (_, i) => ({
    id: `${prefix}${i}`,
    source: { type: 'recording', id: 'talk-rec' },
    sourceInUs: i * (pieceUs + gapUs),
    sourceOutUs: i * (pieceUs + gapUs) + pieceUs,
    speed: 1,
    volume: 1,
    muted: false,
    fadeInUs: 0,
    fadeOutUs: 0,
    look: DEFAULT_LOOK,
    camera: null,
    framing: {},
    effects: [],
  }));
}

/** The talk as a screen recording, with a marker every minute: its markers flag the timeline. */
function recording(markers: number) {
  const marks: Marker[] = Array.from({ length: markers }, (_, i) => ({
    id: `m${i}`,
    atUs: (30 + i * 60) * S,
    kind: i % 3 === 0 ? 'highlight' : 'manual',
  }));
  return {
    id: 'talk-rec',
    kind: 'screen' as const,
    sources: [
      {
        id: 'screen',
        role: 'screen' as const,
        label: 'capture.source.screen',
        runs: [{ assetId: 'talk', offsetUs: 0, durationUs: TWO_HOURS_US, driftPpm: 0 }],
        nudgeUs: 0,
        mix: { volume: 1, muted: false, effects: [] },
      },
    ],
    primary: 'screen',
    markers: marks,
  };
}

function base(id: string, name: string, mediaDir: string): Project {
  return {
    version: 7,
    id,
    name,
    settings: { fps: 30, formats: ['landscape', 'portrait'], primaryFormat: 'landscape' },
    assets: assets(mediaDir),
    recordings: [recording(120)],
    luts: [],
    timeline: { main: [], tracks: [] },
    captions: { enabled: true, style: WORD_POP, place: {}, adSafe: false },
    background: { kind: 'none', color: '#101114', colorEnd: null, assetId: null, padding: 0 },
    audio: { loudness: 'youtube' },
    cursor: CURSOR_STYLE,
    requires: [],
  };
}

const GRID = [
  { x: 0.02, y: 0.02, w: 0.46, h: 0.46 },
  { x: 0.52, y: 0.02, w: 0.46, h: 0.46 },
  { x: 0.02, y: 0.52, w: 0.46, h: 0.46 },
  { x: 0.52, y: 0.52, w: 0.46, h: 0.46 },
];

/**
 * Two hours of talk as 300 pieces of 24 s, captions on in Word pop, 120 markers, and 200 layers: 80 titles (every
 * fourth with motion keys), 40 stickers with motion keys, 40 sound effects, 30 pieces of music (six songs of five 16 s
 * pieces, ducked, each piece with volume keys), 6 zooms and 4 video layers (a 2 × 2 grid at 10:00, two cropped, two
 * with motion keys).
 */
export function twoHourProject(mediaDir = '/media'): Project {
  const p = base('perf-two-hours', 'Perf two hours', mediaDir);
  p.timeline.main = pieces('p', 300, 24 * S);
  const titles = (row: number) =>
    Array.from({ length: 40 }, (_, i) =>
      title(`title-${row}-${i}`, 10 + row * 90 + i * 180, 6, `Part ${i * 2 + row + 1}: the next step`, i % 4 === 0),
    );
  const stickers = Array.from({ length: 40 }, (_, i) =>
    layer(
      `sticker-${i}`,
      45 + i * 180,
      4,
      { type: 'image', assetId: `${PACK}${STICKERS[i % 3]}` },
      layout({ x: 0.78, y: 0.1, w: 0.14, h: 0.25 }, moving(4, true)),
    ),
  );
  const effects = Array.from({ length: 40 }, (_, i) =>
    sound(`sfx-${i}`, 20 + i * 180, SOUND_LEN[i % 3], `${PACK}${SOUNDS[i % 3]}`),
  );
  const music = Array.from({ length: 6 }, (_, song) => songAt(`music-${song}`, 60 + song * 1200, 5, MUSIC[song % 3])).flat();
  const zooms = Array.from({ length: 6 }, (_, i) =>
    layer(
      `zoom-${i}`,
      300 + i * 1200,
      8,
      { type: 'zoom', target: { type: 'rect', rect: { x: 0.25, y: 0.25, w: 0.5, h: 0.5 } }, scale: 2 },
      layout({ x: 0, y: 0, w: 1, h: 1 }),
    ),
  );
  const videos = ['clip-a', 'clip-b', 'clip-c', 'clip-d'].map((assetId, i) =>
    track(`video-${i + 1}`, 'overlay', `Video ${i + 1}`, [
      layer(
        `video-${i + 1}`,
        600,
        60,
        {
          type: 'video',
          assetId,
          sourceInUs: 0,
          speed: 1,
          volume: 1,
          muted: i > 0,
          ...(i % 2 === 0 ? { crop: { left: 0.1, top: 0.05, right: 0.1, bottom: 0.05 } } : {}),
        },
        layout(GRID[i], i % 2 === 1 ? moving(60) : undefined),
      ),
    ]),
  );
  p.timeline.tracks = [
    track('zooms', 'zoom', 'Zoom', zooms),
    ...videos,
    track('stickers', 'overlay', 'Stickers', stickers),
    track('titles-1', 'overlay', 'Titles 1', titles(0)),
    track('titles-2', 'overlay', 'Titles 2', titles(1)),
    track('sounds', 'audio', 'Sounds', effects),
    track('music', 'audio', 'Music', music),
  ];
  return p;
}

/**
 * A 60 s short in 9:16, as one prompt makes it from the talk: 12 cuts of 5 s, Word pop captions, a hook title, the music
 * ducked under the voice with fades and volume keys (four pieces of the song), a whoosh at each cut, three stickers and two zooms.
 */
export function shortProject(mediaDir = '/media'): Project {
  const p = base('perf-short', 'Perf short', mediaDir);
  p.settings = { fps: 30, formats: ['portrait'], primaryFormat: 'portrait' };
  p.timeline.main = pieces('s', 12, 5 * S, 55 * S);
  const hook = title('hook', 0, 3, 'Three steps to a cleaner edit', true);
  const later = [title('step-1', 12, 4, 'Step one', false), title('step-2', 30, 4, 'Step two', false)];
  p.timeline.tracks = [
    track('zooms', 'zoom', 'Zoom', [
      layer('zoom-1', 8, 3, { type: 'zoom', target: { type: 'auto' }, scale: 1.6 }, layout({ x: 0, y: 0, w: 1, h: 1 })),
      layer('zoom-2', 40, 3, { type: 'zoom', target: { type: 'auto' }, scale: 1.6 }, layout({ x: 0, y: 0, w: 1, h: 1 })),
    ]),
    track(
      'stickers',
      'overlay',
      'Stickers',
      [16, 34, 52].map((at, i) =>
        layer(
          `sticker-${i}`,
          at,
          3,
          { type: 'image', assetId: `${PACK}${STICKERS[i]}` },
          layout({ x: 0.7, y: 0.6, w: 0.25, h: 0.14 }, moving(3, true)),
        ),
      ),
    ),
    track('titles', 'overlay', 'Titles', [hook, ...later]),
    track(
      'sounds',
      'audio',
      'Sounds',
      Array.from({ length: 11 }, (_, i) => sound(`whoosh-${i}`, 5 * (i + 1) - 0.3, 0.5, `${PACK}whoosh`)),
    ),
    track('music', 'audio', 'Music', songAt('music', 0, 4, 'upbeat-loop', 12)),
  ];
  return p;
}

/** Plain talk words, said over and over. */
const SAID = 'so this is where it all happens every piece starts as a lump of clay you wedge it first then center it'.split(
  ' ',
);

/**
 * `count` caption words of the talk spread evenly over `overUs` (18,000 over two hours: one every 0.4 s, 0.3 s long),
 * as the caption pages show them.
 */
export function talkWords(count = 18_000, overUs = TWO_HOURS_US): WordView[] {
  const step = overUs / count;
  return Array.from({ length: count }, (_, i) => {
    const startUs = Math.round(i * step);
    const endUs = startUs + Math.round(step * 0.75);
    return {
      text: SAID[i % SAID.length],
      joined: false,
      startUs,
      endUs,
      assetId: 'talk',
      sourceStartUs: startUs,
      sourceEndUs: endUs,
      look: {},
      keyword: null,
    };
  });
}

/** Word pop's pages of `words`: `per` words a page. */
export function talkPages(words: WordView[], per = 3): PageView[] {
  const pages: PageView[] = [];
  for (let i = 0; i < words.length; i += per) {
    const run = words.slice(i, i + per);
    pages.push({ index: pages.length, startUs: run[0].startUs, endUs: run[run.length - 1].endUs, words: run });
  }
  return pages;
}
