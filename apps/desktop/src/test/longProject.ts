import type { Item } from '../bindings/Item';
import type { Project } from '../bindings/Project';
import { fixtureProject, titleItem } from './fixtures';

const S = 1_000_000;

/** A stress case at real scale: one hour as 1,500 cuts of the screen recording and 300 titles on three tracks. */
export function longProject(): Project {
  const p = fixtureProject();
  const [a] = p.timeline.main;
  p.assets = p.assets.map((x) => (x.id === 'screen' ? { ...x, durationUs: 3600 * S } : x));
  p.timeline.main = Array.from({ length: 1500 }, (_, i) => ({
    ...a,
    id: `c${i}`,
    sourceInUs: Math.round(i * 2.4 * S),
    sourceOutUs: Math.round((i + 1) * 2.4 * S),
    camera: null,
  }));
  const titles = (track: number): Item[] =>
    Array.from({ length: 100 }, (_, i) => ({
      ...titleItem,
      id: `t${track}-${i}`,
      attach: null,
      startUs: (i * 36 + track * 12) * S,
      durationUs: 8 * S,
    }));
  p.timeline.tracks = [0, 1, 2].map((t) => ({
    id: `ov${t}`,
    kind: 'overlay' as const,
    name: `Overlay ${t + 1}`,
    hidden: false,
    muted: false,
    locked: false,
    items: titles(t),
  }));
  return p;
}

/**
 * `longProject` with an hour of sound too: 300 one-second sound effects (one every 12 s) on one audio track, and the
 * music as twenty 3-minute pieces back to back on another (ducking set, so they are music).
 */
export function longProjectWithSound(): Project {
  const p = longProject();
  const song = fixtureProject().timeline.tracks.find((t) => t.kind === 'audio')!.items[0];
  const audio = song.content.type === 'audio' ? song.content : null;
  if (!audio) throw new Error('the fixture has no song');
  const effects: Item[] = Array.from({ length: 300 }, (_, i) => ({
    ...song,
    id: `fx${i}`,
    startUs: i * 12 * S,
    durationUs: S,
    content: { ...audio, assetId: 'mic' },
  }));
  const music: Item[] = Array.from({ length: 20 }, (_, i) => ({
    ...song,
    id: `song${i}`,
    startUs: i * 180 * S,
    durationUs: 180 * S,
    content: { ...audio, duck: -12 },
  }));
  const track = (id: string, items: Item[]) => ({
    id,
    kind: 'audio' as const,
    name: id,
    hidden: false,
    muted: false,
    locked: false,
    items,
  });
  p.timeline.tracks = [...p.timeline.tracks, track('sfx', effects), track('bed', music)];
  return p;
}
