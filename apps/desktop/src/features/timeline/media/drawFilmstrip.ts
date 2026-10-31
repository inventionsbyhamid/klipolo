import type { ThumbStrip } from '../../../bindings/ThumbStrip';

/**
 * The most sprites one file's filmstrip draws from in a paint. Zoomed far out, a long clip's tiles would each need
 * their own 64-second chunk (about 50 sprites, 120 MB, for an hour at 1 px/s): instead they share a few chunks' frames.
 */
export const MAX_SPRITES = 8;
/**
 * Decoded sprites the webview keeps: two long files' filmstrips on screen at once (a screen clip and a camera layer),
 * so a repaint never refetches. A 16:9 sprite is 1024 × 576 RGBA, 2.25 MiB decoded: 36 MiB in all (the webview
 * WebContent ≤ 350 MB).
 */
export const CACHED_SPRITES = 2 * MAX_SPRITES;

/** Where a filmstrip draws, in CSS px. */
export interface FilmMap {
  /** The canvas's left edge, from the block's left edge (its start). */
  x0: number;
  widthPx: number;
  heightPx: number;
  /** The block's width: a tile its end cuts shows the middle of what is left of it. */
  blockPx: number;
  /** The source time (µs) chunk groups are sized from: the window's at this zoom, at most the file's (`PaintSpec`). */
  spanUs: number;
  /** Source µs per CSS px at this zoom and speed. */
  usPerPx: number;
  /** The source time (µs) at `xCss` from the block's left edge. */
  sourceAt: (xCss: number) => number;
}

/** One tile: where it draws on the canvas, and its cell in its chunk's sprite. */
export interface Tile {
  x: number;
  w: number;
  chunk: number;
  col: number;
  row: number;
}

const files = new WeakMap<ThumbStrip, Map<number, string>>();

/** A strip's scanned chunks: the sprite file of each, by index. */
export function chunkFiles(strip: ThumbStrip): Map<number, string> {
  let byIndex = files.get(strip);
  if (!byIndex) files.set(strip, (byIndex = new Map(strip.chunks.map((c) => [c.index, c.path]))));
  return byIndex;
}

/** The scanned chunk of group `group` (`size` chunks from `group × size`) nearest its middle; null if none is. */
function representative(scanned: Map<number, string>, group: number, size: number): number | null {
  const [first, middle] = [group * size, group * size + Math.floor(size / 2)];
  for (let d = 0; d < size; d++)
    for (const chunk of [middle - d, middle + d])
      if (chunk >= first && chunk < first + size && scanned.has(chunk)) return chunk;
  return null;
}

/**
 * The tiles over the canvas: laid every `tileW × heightPx / tileH` px from the block's left edge (so they don't move
 * when the canvas does), each showing the one-second slot under the middle of its part inside the block.
 *
 * Zoomed out far enough that one file's tiles in a window could need more than `MAX_SPRITES` chunks, chunks go in
 * groups: aligned to the file, and sized from `spanUs` (the window's source time, at most the file's), so every window
 * step keeps the same groups and sprites. Until then every tile shows its own second: while the window's span plus one
 * tile is at most 7 chunks, its tiles' middles fall in at most 8. A grouped tile shows the frame nearest its own time
 * in its group's representative chunk, held inside the block's own footage: never a frame the user trimmed or cut
 * away. A group with nothing scanned yet draws nothing.
 */
export function filmTiles(strip: ThumbStrip, map: FilmMap): Tile[] {
  const w = (strip.tileW * map.heightPx) / strip.tileH;
  if (!(w > 0)) return [];
  const { perChunk, slotUs } = strip;
  const own = (slot: number) => Math.floor(slot / perChunk);
  // The block's footage, as slots (`sourceAt` holds every x inside it).
  const [first, last] = [
    Math.floor(map.sourceAt(0) / slotUs),
    Math.floor(map.sourceAt(map.blockPx) / slotUs),
  ];
  // A window's tiles span at most its chunks plus a partial one at each end: in groups of that over six, at most eight.
  // The trigger is per zoom (the span and a tile's width), so a window step never switches grouping on or off.
  const chunkUs = slotUs * perChunk;
  const span = map.spanUs / chunkUs;
  const exact = span + (w * map.usPerPx) / chunkUs <= MAX_SPRITES - 1;
  const size = exact ? 1 : Math.ceil((span + 2) / (MAX_SPRITES - 2));
  const scanned = chunkFiles(strip);
  const tiles: Tile[] = [];
  const end = Math.ceil((map.x0 + map.widthPx) / w);
  for (let k = Math.max(0, Math.floor(map.x0 / w)); k < end; k++) {
    const middle = (k * w + Math.max(k * w, Math.min((k + 1) * w, map.blockPx))) / 2;
    const at = Math.min(last, Math.max(first, Math.floor(map.sourceAt(middle) / slotUs)));
    let slot = at;
    if (size > 1) {
      const chunk = representative(scanned, Math.floor(own(at) / size), size);
      if (chunk === null) continue;
      const nearest = chunk < own(at) ? (chunk + 1) * perChunk - 1 : chunk > own(at) ? chunk * perChunk : at;
      slot = Math.min(last, Math.max(first, nearest));
      // Held inside the footage it can land in the tile's own chunk: its own slot is then as cheap, and exact.
      if (own(slot) === own(at)) slot = at;
      if (!scanned.has(own(slot))) continue;
    }
    const cell = slot % perChunk;
    tiles.push({
      x: k * w - map.x0,
      w,
      chunk: own(slot),
      col: cell % strip.cols,
      row: Math.floor(cell / strip.cols),
    });
  }
  return tiles;
}

/**
 * The strip's tiles from the decoded sprites by chunk. A chunk that isn't scanned or decoded yet draws nothing, so
 * the block's colour shows there. Every scanned chunk has every slot (the engine fills a slot without a
 * keyframe from the nearest earlier one), so a tile never needs another chunk.
 */
export function drawFilmstrip(
  ctx: CanvasRenderingContext2D,
  strip: ThumbStrip,
  images: ReadonlyMap<number, CanvasImageSource>,
  map: FilmMap,
) {
  for (const tile of filmTiles(strip, map)) {
    const img = images.get(tile.chunk);
    if (!img) continue;
    ctx.drawImage(
      img,
      tile.col * strip.tileW,
      tile.row * strip.tileH,
      strip.tileW,
      strip.tileH,
      tile.x,
      0,
      tile.w,
      map.heightPx,
    );
  }
}
