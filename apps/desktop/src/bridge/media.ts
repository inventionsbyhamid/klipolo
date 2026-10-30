import { invoke } from '@tauri-apps/api/core';
import type { PeaksInfo } from '../bindings/PeaksInfo';
import type { ThumbStrip } from '../bindings/ThumbStrip';
import { call } from './call';

/** What the timeline draws on its blocks: a file's waveform peaks and thumbnail strip (read-only). */
export const media = {
  peaksInfo: (path: string) => call<PeaksInfo | null>('media_peaks_info', { path }),
  /** Raw i16 [min, max] per channel per bucket of `level`, from bucket `from`; null where there is no media cache
   * (the dev fake), so callers keep the block's plain colour. */
  peaks: (path: string, level: number, from: number, count: number) =>
    invoke<ArrayBuffer | null>('media_peaks', { path, level, from, count }),
  thumbs: (path: string) => call<ThumbStrip | null>('media_thumbs', { path }),
  /** How loud the file is on each channel (its loudest sample, 0 to 1); null until its sound cache is built. */
  channelPeaks: (path: string) => call<number[] | null>('media_channel_peaks', { path }),
};
