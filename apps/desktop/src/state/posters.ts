import { create } from 'zustand';
import type { ThumbTile } from '../bindings/ThumbTile';
import { mediaUrl } from '../bridge/commands';
import { projects } from '../bridge/projects';
import { preloadImage } from '../lib/preloadImage';

/** A card's poster as it landed: its picture, and which poster heard it was (`heard`), to tell what list it is newer than. */
export interface LandedPoster {
  thumb: ThumbTile;
  heard: number;
}

/**
 * The posters Rust made since the page loaded, by project file (`projects.onPoster`), each kept once its picture has
 * loaded, so a card that swaps to it never shows an empty picture between. `heard` counts the posters heard so far: a
 * list asked for after a poster was heard already shows it (`newerThan`).
 */
export const usePosters = create<{ landed: Record<string, LandedPoster>; heard: number }>(() => ({
  landed: {},
  heard: 0,
}));

let following: Promise<void> | null = null;

/** Hears each card's poster from Rust, once per page. */
export function followPosters(): Promise<void> {
  following ??= projects
    .onPoster((poster) => {
      const heard = usePosters.getState().heard + 1;
      usePosters.setState({ heard });
      preloadImage(mediaUrl(poster.thumb.sprite)).then(
        () =>
          usePosters.setState((now) => ({ landed: { ...now.landed, [poster.path]: { thumb: poster.thumb, heard } } })),
        // It didn't load: the card keeps the picture it has.
        () => {},
      );
    })
    .then(
      () => {},
      // No events here (a page without Rust): the cards keep the pictures listed.
      () => {},
    );
  return following;
}

/** The poster heard for `path` after the list read when `heardBefore` posters had been heard; null for none. */
export function newerThan(landed: Record<string, LandedPoster>, path: string, heardBefore: number): ThumbTile | null {
  const poster = landed[path];
  return poster && poster.heard > heardBefore ? poster.thumb : null;
}
