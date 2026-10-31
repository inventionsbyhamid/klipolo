import { useEffect, useState } from 'react';
import { create } from 'zustand';
import type { Asset } from '../../bindings/Asset';
import type { PackItem } from '../../bindings/PackItem';
import type { Project } from '../../bindings/Project';
import { library } from '../../bridge/library';
import { IMAGE_US } from '../../lib/placement';
import { MUSIC_MIN_US } from './useImportMedia';

/** The id a pack item's asset has in a project (`crates/klipolo-app/src/pack.rs`). */
export const packAssetId = (id: string) => `klipolo-pack-${id}`;

/** The tool a picture was made with ("Muse Image"): its own word, or the folder Muse Image saves into. */
export function madeWith(asset: Asset): string | null {
  if (asset.madeWith) return asset.madeWith;
  // Made before the asset said so: `<project> Media/Muse Image/<file>`.
  return /(^|\/)[^/]* Media\/Muse Image\/[^/]+$/.test(asset.path) ? 'Muse Image' : null;
}

/** What the timeline uses of each file: as a clip, music, a sound effect or a layer. */
export interface Uses {
  any: Set<string>;
  music: Set<string>;
  sounds: Set<string>;
}

const usesOf = new WeakMap<Project['timeline'], Uses>();

/** The files `project` puts on its timeline, worked out once per timeline. */
export function uses(project: Project): Uses {
  let found = usesOf.get(project.timeline);
  if (found) return found;
  found = { any: new Set(), music: new Set(), sounds: new Set() };
  for (const clip of project.timeline.main) if (clip.source.type === 'asset') found.any.add(clip.source.id);
  for (const track of project.timeline.tracks)
    for (const item of track.items) {
      const c = item.content;
      if (!('assetId' in c)) continue;
      found.any.add(c.assetId);
      if (c.type === 'audio') (typeof c.duck === 'number' ? found.music : found.sounds).add(c.assetId);
    }
  usesOf.set(project.timeline, found);
  return found;
}

/**
 * The person's own files on each tab: videos on Clips; a sound on Music when it plays as music or is long
 * enough to be a song, on Sounds when it plays as an effect or is short (both, if it plays as both); pictures on
 * Images. The pack's own files are listed with the pack, not here.
 */
export function shelves(project: Project) {
  const own = project.assets.filter((a) => !a.id.startsWith('klipolo-pack-'));
  const used = uses(project);
  const audio = own.filter((a) => a.kind === 'audio');
  return {
    clips: own.filter((a) => a.kind === 'video'),
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

/** Klipolo's pack, read once per page (empty until Rust answers, and where it can't). */
export const usePack = create<{ items: PackItem[]; asked: boolean }>(() => ({ items: [], asked: false }));

export function loadPack() {
  if (usePack.getState().asked) return;
  usePack.setState({ asked: true });
  void library
    .pack()
    .then((items) => usePack.setState({ items }))
    .catch(() => undefined);
}

const pictures = new Map<string, Promise<string | null>>();

/**
 * A picture for the Images tab, as a blob URL: a pack sticker (`packId`) or a picture the project uses (`path`). Asked
 * once per picture; null until it is here, and where it can't be read.
 */
export function usePicture(from: { packId: string } | { path: string }): string | null {
  const key = 'packId' in from ? `pack:${from.packId}` : from.path;
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    let asked = pictures.get(key);
    if (!asked) {
      asked = library
        .picture(from)
        .then((bytes) => URL.createObjectURL(new Blob([bytes])))
        .catch(() => null);
      pictures.set(key, asked);
    }
    void asked.then((found) => {
      if (live) setUrl(found);
    });
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the key names the picture
  }, [key]);
  return url;
}
