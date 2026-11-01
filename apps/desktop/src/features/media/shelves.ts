import { useEffect, useState, type RefObject } from 'react';
import { create } from 'zustand';
import type { Asset } from '../../bindings/Asset';
import type { ClipSource } from '../../bindings/ClipSource';
import type { PackItem } from '../../bindings/PackItem';
import type { Project } from '../../bindings/Project';
import type { Recording } from '../../bindings/Recording';
import { codedMessage } from '../../bridge/call';
import { library } from '../../bridge/library';
import { IMAGE_US } from '../../lib/placement';
import { firstAsset, primarySource } from '../../lib/recording';
import { assetById, assetName } from '../../lib/timeline';
import { useProject } from '../../state/project';
import { fileKey } from './marks';
import { recordingFiles, recordingRows, type RecordingRow } from './recordings';
import { MUSIC_MIN_US } from './useImportMedia';

/** The id a pack item's asset has in a project (`crates/klipolo-app/src/pack.rs`). */
export const packAssetId = (id: string) => `klipolo-pack-${id}`;

/** The tool a picture was made with ("Muse Image"): its own word, or the folder Muse Image saves into. */
export function madeWith(asset: Asset): string | null {
  if (asset.madeWith) return asset.madeWith;
  // Made before the asset said so: `<project> Media/Muse Image/<file>`.
  return /(^|\/)[^/]* Media\/Muse Image\/[^/]+$/.test(asset.path) ? 'Muse Image' : null;
}

/** What the timeline uses of each file: as a clip, music, a sound effect or a layer; and the recordings it plays. */
export interface Uses {
  any: Set<string>;
  music: Set<string>;
  sounds: Set<string>;
  recordings: Set<string>;
  /**
   * The files played on their own: a main clip or a layer of the file itself, not a recording's source separated onto
   * a track (a layer tied to its recording).
   */
  alone: Set<string>;
}

const usesOf = new WeakMap<Project['timeline'], Uses>();

/** The files and recordings `project` puts on its timeline, worked out once per timeline. */
export function uses(project: Project): Uses {
  let found = usesOf.get(project.timeline);
  if (found) return found;
  found = { any: new Set(), music: new Set(), sounds: new Set(), recordings: new Set(), alone: new Set() };
  for (const clip of project.timeline.main) {
    if (clip.source.type === 'recording') found.recordings.add(clip.source.id);
    else {
      found.any.add(clip.source.id);
      found.alone.add(clip.source.id);
    }
  }
  for (const track of project.timeline.tracks)
    for (const item of track.items) {
      const c = item.content;
      if (!('assetId' in c)) continue;
      found.any.add(c.assetId);
      if (!('tie' in c && c.tie)) found.alone.add(c.assetId);
      if (c.type === 'audio') (typeof c.duck === 'number' ? found.music : found.sounds).add(c.assetId);
    }
  usesOf.set(project.timeline, found);
  return found;
}

/**
 * One item on the Clips tab: a video file, or a whole recording (whose files are listed on their own only while the
 * video plays one alone).
 */
export interface ShelfClip {
  /** What it is: the file, or the recording. */
  source: ClipSource;
  name: string;
  /**
   * What its drag carries (its kind, name and length): the file, or a recording's main footage named and timed as the
   * whole take. Its path is the file its picture comes from.
   */
  asset: Asset;
  /** A recording's length and sources ("1:00, screen and mic"); none for a file, whose row shows its length. */
  meta?: string;
  /** Words a search finds it by besides its name: a recording's sources ("Screen", "Mic"). */
  tags: string[];
  /** Its favorite and recently used mark: its file's, a recording's main footage's. */
  key: string;
}

const fileClip = (asset: Asset): ShelfClip => ({
  source: { type: 'asset', id: asset.id },
  name: asset.name,
  asset,
  tags: [],
  key: fileKey(asset.path),
});

/** Recording `row` as a Clips item, standing for its main footage `footage` (none: its file is gone). */
function recordingClip(row: RecordingRow, recording: Recording, footage: Asset | null): ShelfClip {
  const asset: Asset = {
    ...(footage ?? { id: row.id, kind: 'video', path: '', width: 0, height: 0, fps: 0, hasAudio: false }),
    name: row.name,
    durationUs: row.durationUs,
  };
  return {
    source: { type: 'recording', id: row.id },
    name: row.name,
    asset,
    meta: row.meta,
    tags: recording.sources.map((s) => codedMessage({ code: s.label, params: {} }) || s.label),
    key: fileKey(asset.path),
  };
}

/**
 * The person's own files on each tab: videos and recordings on Clips, a recording as one item for the
 * whole take; a sound on Music when it plays as music or is long enough to be a song, on Sounds when it plays as an
 * effect or is short (both, if it plays as both); pictures on Images. A recording's files (`recordingFiles`) are
 * listed only while the video plays one on its own (a clip or a layer of that file, not its source separated onto a
 * track), named by their source and recording ("Camera of Recording 1"). The pack's own files are listed with the
 * pack, not here.
 */
export function shelves(project: Project) {
  const taken = recordingFiles(project);
  const used = uses(project);
  const own = project.assets
    .filter((a) => !a.id.startsWith('klipolo-pack-') && (!taken.has(a.id) || used.alone.has(a.id)))
    .map((a) => (taken.has(a.id) ? { ...a, name: assetName(project, a) } : a));
  const audio = own.filter((a) => a.kind === 'audio');
  // Clips in the order the video plays them (first use), then those not on it yet, as imported (a recording where its
  // first file came in).
  const order = new Map<string, number>();
  project.timeline.main.forEach((clip, i) => {
    const key = `${clip.source.type}:${clip.source.id}`;
    if (!order.has(key)) order.set(key, i);
  });
  const imported = new Map(project.assets.map((a, i) => [a.id, i]));
  const rows = recordingRows(project);
  const clips = [
    ...own.filter((a) => a.kind === 'video').map((a) => ({ clip: fileClip(a), at: imported.get(a.id) ?? Infinity })),
    ...project.recordings.map((recording, i) => {
      const files = recording.sources.flatMap((s) => s.runs.map((run) => imported.get(run.assetId) ?? Infinity));
      const footage = assetById(project, firstAsset(primarySource(recording)));
      return { clip: recordingClip(rows[i], recording, footage), at: Math.min(Infinity, ...files) };
    }),
  ];
  const place = (c: ShelfClip) => order.get(`${c.source.type}:${c.source.id}`) ?? Infinity;
  return {
    clips: clips
      .sort((a, b) => place(a.clip) - place(b.clip) || a.at - b.at)
      .map(({ clip }) => clip),
    music: audio.filter((a) => used.music.has(a.id) || (!used.sounds.has(a.id) && a.durationUs >= MUSIC_MIN_US)),
    sounds: audio.filter((a) => used.sounds.has(a.id) || (!used.music.has(a.id) && a.durationUs < MUSIC_MIN_US)),
    images: own.filter((a) => a.kind === 'image' && !madeWith(a)),
    made: own.filter((a) => a.kind === 'image' && !!madeWith(a)),
  };
}

/** A pack item as an asset to drag (its length and kind for the drop's ghost): the same id the project gives it. */
export function packAsset(item: PackItem): Asset {
  return {
    id: packAssetId(item.id),
    kind: item.category === 'sticker' ? 'image' : 'audio',
    path: '',
    name: item.name,
    durationUs: item.category === 'sticker' ? IMAGE_US : item.durationUs,
    width: item.width,
    height: item.height,
    fps: 0,
    hasAudio: item.category !== 'sticker',
  };
}

/** Klipolo's pack, read once per page (empty until Rust answers; asked again by the next library tab where it can't). */
export const usePack = create<{ items: PackItem[]; asked: boolean }>(() => ({ items: [], asked: false }));

export function loadPack() {
  if (usePack.getState().asked) return;
  usePack.setState({ asked: true });
  void library
    .pack()
    .then((items) => usePack.setState({ items }))
    .catch(() => usePack.setState({ asked: false }));
}

/** The pictures asked for, by key: a blob URL once it is here. A picture that couldn't be had isn't kept. */
const pictures = new Map<string, Promise<string | null>>();
/** Bumped as the pictures are let go, so tiles showing one ask again. */
const usePictures = create<{ epoch: number }>(() => ({ epoch: 0 }));

/** Lets every picture go (their blob URLs revoked): another project opened, so its tiles ask for theirs. */
export function forgetPictures() {
  for (const asked of pictures.values()) void asked.then((url) => url && URL.revokeObjectURL(url));
  pictures.clear();
  usePictures.setState((s) => ({ epoch: s.epoch + 1 }));
}

/** Which opening of which project the copy holds. */
const opening = (p: { snapshot: { project: { id: string }; opening?: number } | null }) =>
  p.snapshot ? `${p.snapshot.project.id}:${p.snapshot.opening ?? ''}` : '';
useProject.subscribe((s, prev) => {
  if (opening(s) !== opening(prev)) forgetPictures();
});

/**
 * Whether the element in `ref` is on screen or near it (within 200 px): a tile asks for its picture only then, so a
 * long grid reads the pictures it shows. Stays true once it was.
 */
export function useNearView(ref: RefObject<Element | null>): boolean {
  const [near, setNear] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || near) return;
    if (typeof IntersectionObserver === 'undefined') {
      setNear(true);
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        setNear(true);
        observer.disconnect();
      },
      { rootMargin: '200px' },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref, near]);
  return near;
}

/**
 * A picture for the Images tab, as a blob URL of its thumbnail (Rust sends one at most 512 px on its long side): a
 * pack sticker (`packId`) or a picture the project uses (`path`). Asked once `near` (its tile is near the view), once
 * per picture while it is had; null until it is here, and where it can't be read (a later tile asks again).
 */
export function usePicture(from: { packId: string } | { path: string }, near = true): string | null {
  const key = 'packId' in from ? `pack:${from.packId}` : from.path;
  const epoch = usePictures((s) => s.epoch);
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    setUrl(null);
    if (!near) return;
    let live = true;
    let asked = pictures.get(key);
    if (!asked) {
      asked = library
        .picture(from)
        .then((bytes) => URL.createObjectURL(new Blob([bytes], { type: 'image/png' })))
        .catch(() => {
          pictures.delete(key);
          return null;
        });
      pictures.set(key, asked);
    }
    void asked.then((found) => {
      if (live) setUrl(found);
    });
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the key names the picture
  }, [key, near, epoch]);
  return url;
}
