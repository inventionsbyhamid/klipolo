import { useEffect, useState, type CSSProperties } from 'react';
import type { ThumbTile } from '../../bindings/ThumbTile';
import { mediaUrl } from '../../bridge/commands';
import { useMediaVersion } from '../../state/media';
import { stripOf } from '../timeline/media/spriteCache';

/**
 * The first thumbnail of the video at `path` (tile 0 of its strip's chunk 0, which the media scan writes first), asked
 * for once `near` (on screen or close to it) and again whenever the file's thumbnails change. The strip is the one the
 * timeline's filmstrips read (`stripOf`), asked for once for both. Null until it is scanned, where there is no media
 * cache, or for no file (`path` empty); asked for another file, null until that one's is here.
 */
export function useClipPicture(path: string, near = true): ThumbTile | null {
  const version = useMediaVersion(path || null);
  const [shown, setShown] = useState<{ path: string; tile: ThumbTile } | null>(null);
  useEffect(() => {
    if (!near || !path) return;
    let live = true;
    stripOf(path, version).then(
      (strip) => {
        const first = strip?.chunks.find((chunk) => chunk.index === 0);
        if (!live || !strip || !first) return;
        setShown({ path, tile: { sprite: first.path, tileW: strip.tileW, tileH: strip.tileH, cols: strip.cols } });
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [path, version, near]);
  return shown?.path === path ? shown.tile : null;
}

/**
 * Tile 0 of `tile`'s sheet filling a square box, centred, as a picture is cropped to cover it: a wide tile is as tall
 * as the box and wider, a tall one as wide and taller (the box hides the rest). The sheet is drawn `cols` times the
 * tile's width from its top left, so the tile fills the element.
 */
export function squareTileStyle(tile: ThumbTile): CSSProperties {
  const aspect = tile.tileW > 0 && tile.tileH > 0 ? tile.tileW / tile.tileH : 1;
  return {
    width: `${Math.max(1, aspect) * 100}%`,
    height: `${Math.max(1, 1 / aspect) * 100}%`,
    backgroundImage: `url("${mediaUrl(tile.sprite)}")`,
    backgroundSize: `${tile.cols * 100}% auto`,
    backgroundPosition: '0px 0px',
  };
}
