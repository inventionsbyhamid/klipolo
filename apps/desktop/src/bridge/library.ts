import { invoke } from '@tauri-apps/api/core';
import type { DropOnto } from '../bindings/DropOnto';
import type { ItemPatch } from '../bindings/ItemPatch';
import type { Mark } from '../bindings/Mark';
import type { MediaSource } from '../bindings/MediaSource';
import type { PackItem } from '../bindings/PackItem';
import type { PlaceAnswer } from '../bindings/PlaceAnswer';
import type { RangeUs } from '../bindings/RangeUs';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import type { TextKind } from '../bindings/TextKind';
import { applyBase } from './applyBase';
import { call } from './call';

/** The library's favorites and recently used items, by key (`pack:<id>`, `file:<path>`). */
export type LibraryMarks = Partial<Record<string, Mark>>;

/**
 * The media commands (`crates/klipolo-app/src/commands/media.rs`): Rust builds the ops (music that fits the video, a
 * sound effect, a picture, a text preset or title template) from the open project as it is, and they go in as one undo
 * step. Each goes with the copy's base (`applyBase`), as an edit does, and returns what changed.
 */
export const library = {
  addMusic: (source: MediaSource, startUs: number) =>
    call<SnapshotPatch>('media_add_music', { source, startUs, ...applyBase() }),
  addSound: (source: MediaSource, atUs: number) =>
    call<SnapshotPatch>('media_add_sound', { source, atUs, ...applyBase() }),
  addImage: (source: MediaSource, atUs: number) =>
    call<SnapshotPatch>('media_add_image', { source, atUs, ...applyBase() }),
  /**
   * Sounds and pictures let go on the timeline: one after another from `atUs`, on the track `onto` names. Says which
   * files came in with no layer, and why.
   */
  place: (sources: MediaSource[], atUs: number, onto: DropOnto) =>
    call<PlaceAnswer>('media_place', { sources, atUs, onto, ...applyBase() }),
  addText: (kind: TextKind, atUs: number, text: string | null = null) =>
    call<SnapshotPatch>('media_add_text', { kind, atUs, text, ...applyBase() }),
  updateItem: (id: string, patch: ItemPatch, label: string) =>
    call<SnapshotPatch>('media_update_item', { id, patch, label, ...applyBase() }),
  /** Klipolo's pack: its sounds, music beds and stickers. */
  pack: () => call<PackItem[]>('library_pack'),
  /** The library's favorites and recently used items, kept for every project. */
  marks: () => call<LibraryMarks>('library_marks'),
  /** Stars item `key` or takes its star off; the marks as saved. */
  setFavorite: (key: string, on: boolean) => call<LibraryMarks>('library_set_favorite', { key, on }),
  /** A picture's bytes: a pack sticker (`packId`) or one the open project uses (`path`). */
  picture: (from: { packId: string } | { path: string }) =>
    invoke<ArrayBuffer>('media_picture', 'packId' in from ? { packId: from.packId } : { path: from.path }),
  /** Where someone speaks on the timeline, as the mix ducks music under it. */
  speechRanges: () => call<RangeUs[]>('media_speech_ranges'),
};
