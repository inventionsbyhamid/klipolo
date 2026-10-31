import type { AssetKind } from '../bindings/AssetKind';

/**
 * What a file Klipolo imports is, by its extension, as the import reads it (`crates/klipolo-app/src/media.rs`): a GIF
 * plays as a video, an MP3 with a cover picture is a sound. Null for anything else.
 */
export const EXTENSION_KINDS: Record<string, AssetKind> = {
  mp4: 'video',
  mov: 'video',
  m4v: 'video',
  mkv: 'video',
  webm: 'video',
  avi: 'video',
  gif: 'video',
  mp3: 'audio',
  wav: 'audio',
  m4a: 'audio',
  aac: 'audio',
  flac: 'audio',
  ogg: 'audio',
  png: 'image',
  jpg: 'image',
  jpeg: 'image',
  webp: 'image',
  heic: 'image',
  avif: 'image',
  tif: 'image',
  tiff: 'image',
};

export function kindOfPath(path: string): AssetKind | null {
  const dot = path.lastIndexOf('.');
  if (dot < 0 || dot < path.lastIndexOf('/')) return null;
  return EXTENSION_KINDS[path.slice(dot + 1).toLowerCase()] ?? null;
}
