import { create } from 'zustand';
import { lastFrameUs } from '../lib/time';
import { videoDurationUs } from '../lib/timeline';
import { clockAnchor, clockUs, onClock } from './playClock';
import { setLoopRule, usePlayback } from './playback';
import { useProject } from './project';
import { useRange, type TimeRange } from './range';

interface LoopStore {
  /** Playback loops: the selected time range, else the whole video. */
  on: boolean;
  toggle: () => void;
}

/** Loop playback (the player bar's Loop, ⌘L): off when Klipolo opens. */
export const useLoop = create<LoopStore>((set) => ({
  on: false,
  toggle: () => set((s) => ({ on: !s.on })),
}));

/**
 * What plays round while looping: the selected time range, else the whole video, ending at the video's last frame
 * (where playback comes to rest). Null when looping is off or there is no video.
 */
function loopSpan(): TimeRange | null {
  const snapshot = useProject.getState().snapshot;
  if (!useLoop.getState().on || !snapshot) return null;
  const last = lastFrameUs(videoDurationUs(snapshot.project.timeline), snapshot.project.settings.fps);
  const range = useRange.getState().range;
  if (range && range.startUs < last) return { startUs: range.startUs, endUs: Math.min(range.endUs, last) };
  return last > 0 ? { startUs: 0, endUs: last } : null;
}

/**
 * A run asked to start at `us` starts there when it is inside the loop, else at the loop's start (the playhead outside
 * the range, or at the video's end); the engine's end of the video takes a run that reached it back round.
 */
setLoopRule({
  startAt: (us) => {
    const span = loopSpan();
    return !span || (us >= span.startUs && us < span.endUs) ? us : span.startUs;
  },
  again: () => loopSpan() !== null,
});

/**
 * While looping a range that ends before the video does, a forward run goes back to the range's start each time the
 * clock reaches the range's end. A timer waits for that moment from the clock's anchor, and is set again whenever the
 * clock moves (a play, a pause, a seek, a re-anchor), the range changes or Loop is turned on or off. Only a clock that
 * reaches the end turns: one already past it (turned on there, or a seek past it) plays on to the video's end, which
 * loops back to the range. A frame that re-anchors the clock past the end before the timer fires still turns. Returns
 * the stop.
 */
export function keepLooping(): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  /** The range end the timer waits for: the clock was before it. */
  let due: number | null = null;
  const arm = () => {
    clearTimeout(timer);
    const was = due;
    due = null;
    const span = loopSpan();
    const range = useRange.getState().range;
    const anchor = clockAnchor();
    // Only a range that ends before the video does: the video's own end loops through the engine.
    if (!span || !range || span.endUs !== range.endUs || !anchor.running || anchor.rate <= 0) return;
    if (!usePlayback.getState().playing) return;
    const now = clockUs();
    if (now >= span.endUs) {
      if (was === span.endUs) usePlayback.getState().seek(span.startUs);
      return;
    }
    due = span.endUs;
    timer = setTimeout(arm, (span.endUs - now) / anchor.rate / 1000);
  };
  const stops = [
    onClock(arm),
    useRange.subscribe((s, prev) => {
      if (s.range !== prev.range) arm();
    }),
    useLoop.subscribe((s, prev) => {
      if (s.on !== prev.on) arm();
    }),
  ];
  return () => {
    clearTimeout(timer);
    for (const stop of stops) stop();
  };
}
