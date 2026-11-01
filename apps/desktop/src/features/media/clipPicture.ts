import { useEffect, useState, type CSSProperties } from 'react';
import type { ThumbStrip } from '../../bindings/ThumbStrip';
import type { ThumbTile } from '../../bindings/ThumbTile';
import { mediaUrl } from '../../bridge/commands';
import { media } from '../../bridge/media';
import { useMediaVersion } from '../../state/media';

/** Each file's thumbnail strip as last asked, with the media version it was asked at. */
const strips = new Map<string, { version: number; strip: Promise<ThumbStrip | null> }>();

/** `path`'s strip at media version `version`: asked once per version, and again later where it couldn't be had. */
function stripAt(path: string, version: number): Promise<ThumbStrip | null> {
  const had = strips.get(path);
  if (had?.version === version) return had.strip;
  const strip = media.thumbs(path).catch(() => {
    if (strips.get(path)?.strip === strip) strips.delete(path);
    return null;
  });
  strips.set(path, { version, strip });
  return strip;
}

/**
 * The first thumbnail of the video at `path` (tile 0 of its strip's chunk 0, which the media scan writes first), asked
 * for once `near` (on screen or close to it) and again whenever the file's thumbnails change; null until it is
 * scanned, where there is no media cache, or for no file (`path` empty).
 */
export function useClipPicture(path: string, near = true): ThumbTile | null {
  const version = useMediaVersion(path || null);
  const [tile, setTile] = useState<ThumbTile | null>(null);
  useEffect(() => {
    if (!near || !path) return;
    let live = true;
    void stripAt(path, version).then((strip) => {
      const first = strip?.chunks.find((chunk) => chunk.index === 0);
      if (!live || !strip || !first) return;
      setTile({ sprite: first.path, tileW: strip.tileW, tileH: strip.tileH, cols: strip.cols });
    });
    return () => {
      live = false;
    };
  }, [path, version, near]);
  return tile;
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
