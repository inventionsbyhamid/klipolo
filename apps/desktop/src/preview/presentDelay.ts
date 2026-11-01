import { FLAG_PLAYBACK, type FrameMeta } from './frameTrailer';

/** How often (ms) the measured delay goes to the engine while playing. */
const REPORT_MS = 1000;
/** The fewest frames a report is the median of. */
const MIN_FRAMES = 10;

/**
 * The socket presenter's delivery time. `note` takes each arriving frame; for a playing one it notes the time from the
 * engine sending it to its arrival (wall clock, as the engine stamps frames). About once a second while playing it
 * returns their median for the engine, which renders frames that far ahead of the sound (they wait in the queue until
 * due), so picture and sound line up; null otherwise.
 */
export function presentDelay(): (meta: FrameMeta) => number | null {
  const delays: number[] = [];
  let lastReport = 0;
  return (meta) => {
    if ((meta.flags & FLAG_PLAYBACK) === 0) return null;
    const now = Date.now();
    delays.push(now - meta.sentMs);
    if (now - lastReport < REPORT_MS || delays.length < MIN_FRAMES) return null;
    lastReport = now;
    delays.sort((a, b) => a - b);
    const median = delays[delays.length >> 1];
    delays.length = 0;
    return median;
  };
}
