import { Channel } from '@tauri-apps/api/core';
import type { ReversedCopy } from '../bindings/ReversedCopy';
import { call } from './call';

/**
 * Reverse (`crates/klipolo-app/src/commands/reverse.rs`): a clip's footage written backward to a file of its own beside
 * the project. The page then plays the clip from it (`reverseClip`, with the new file, as one undo step).
 */
export const reverser = {
  /** Writes clip `clipId` backward; `onProgress` hears 0 to 1. Resolves with the copy once it is whole. */
  make: (clipId: string, onProgress: (done: number) => void) => {
    const progress = new Channel<number>();
    progress.onmessage = onProgress;
    return call<ReversedCopy>('clip_reverse', { clipId, progress });
  },
  /** Stops clip `clipId`'s copy: its `make` fails with `reverse.stopped`, and nothing is left on the disk. */
  stop: (clipId: string) => call<void>('clip_reverse_stop', { clipId }),
  /** Deletes reversed copy `path` the page didn't use (only a reversed copy, never one the open project plays). */
  discard: (path: string) => call<void>('clip_reverse_discard', { path }),
};
