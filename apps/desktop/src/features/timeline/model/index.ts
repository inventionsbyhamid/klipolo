import type { Clip } from '../../../bindings/Clip';
import type { Item } from '../../../bindings/Item';
import { clipDurationUs } from '../../../lib/timeline';

/** Where each block starts and ends (µs), in order; ends never decrease. */
export interface Spans {
  starts: Float64Array;
  ends: Float64Array;
}

export interface MainIndex extends Spans {
  /** Where the main track ends. The video may run longer, to its last layer (`videoDurationUs`). */
  mainEndUs: number;
}

const mains = new WeakMap<Clip[], MainIndex>();
const items = new WeakMap<Item[], Spans>();

/** The main track's spans, built once per array (patches keep an unchanged track's array). */
export function mainIndex(main: Clip[]): MainIndex {
  let index = mains.get(main);
  if (!index) {
    const starts = new Float64Array(main.length);
    const ends = new Float64Array(main.length);
    let t = 0;
    for (let i = 0; i < main.length; i++) {
      starts[i] = t;
      t += clipDurationUs(main[i]);
      ends[i] = t;
    }
    index = { starts, ends, mainEndUs: t };
    mains.set(main, index);
  }
  return index;
}

/** A track's spans (its items are sorted and don't overlap), built once per array. */
export function itemSpans(list: Item[]): Spans {
  let spans = items.get(list);
  if (!spans) {
    const starts = new Float64Array(list.length);
    const ends = new Float64Array(list.length);
    list.forEach((item, i) => {
      starts[i] = item.startUs;
      ends[i] = item.startUs + item.durationUs;
    });
    spans = { starts, ends };
    items.set(list, spans);
  }
  return spans;
}

/** The clip playing at `us` (start ≤ us < end), or −1. */
export function clipIndexAt(index: MainIndex, us: number): number {
  let lo = 0;
  let hi = index.starts.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (index.starts[mid] <= us) lo = mid + 1;
    else hi = mid;
  }
  const i = lo - 1;
  return i >= 0 && us < index.ends[i] ? i : -1;
}
