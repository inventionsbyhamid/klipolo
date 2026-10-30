import { useLayoutEffect, useRef } from 'react';
import { formatTime } from '../../player/time';
import { clockAnchor, clockUs, onClock } from '../../state/playClock';
import { usePlayback } from '../../state/playback';

/** The timecode shows tenths, so while playing it is read ten times a second. */
const PLAYING_EVERY_MS = 100;

/**
 * The playhead's time ("0:02.1", the player bar's `m:ss.d`). Outside React: paused, it follows the store; playing, it
 * reads the clock ten times a second and writes the text only when it changes (never a render or a DOM write per
 * frame).
 */
export function Timecode({ durationUs, className }: { durationUs: number; className?: string }) {
  const ref = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    let shown = '';
    let timer: ReturnType<typeof setInterval> | undefined;
    const write = () => {
      const { playing, playheadUs } = usePlayback.getState();
      // Never past the end: an undo can make the video shorter than where the playhead was.
      const us = Math.min(playing && clockAnchor().running ? clockUs() : playheadUs, durationUs);
      const text = formatTime(us);
      if (text !== shown) el.textContent = shown = text;
    };
    const sync = () => {
      clearInterval(timer);
      timer = usePlayback.getState().playing ? setInterval(write, PLAYING_EVERY_MS) : undefined;
      write();
    };
    sync();
    const offClock = onClock(write);
    const offStore = usePlayback.subscribe((s, prev) => {
      if (s.playing !== prev.playing) sync();
      else if (!s.playing && s.playheadUs !== prev.playheadUs) write();
    });
    return () => {
      clearInterval(timer);
      offClock();
      offStore();
    };
  }, [durationUs]);
  return <span ref={ref} className={className} />;
}
