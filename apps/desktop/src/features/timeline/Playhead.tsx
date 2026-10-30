import { useLayoutEffect, useRef } from 'react';
import { cn } from '../../design/cn';
import { clockAnchor, clockUs, onClock } from '../../state/playClock';
import { usePlayback } from '../../state/playback';
import { usToPx } from './scale';

/** How far one playback animation runs: far past any clip (the engine's stop or a re-anchor replaces it first). */
const HORIZON_US = 3_600_000_000;

/**
 * The paper playhead at the current time (never red): a 2 px line across the lanes, or (`head`) its
 * 12 x 14 px head on the ruler.
 * Outside React, and with no work per frame: paused, a store subscription sets its `transform`; playing, one linear
 * Web Animations transform animation moves it on the compositor from the clock's anchor at the clock's rate (backward
 * it comes to rest at the start), restarted only when the engine's frames re-anchor the clock.
 */
export function Playhead({ pps, head = false }: { pps: number; head?: boolean }) {
  const ref = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const half = head ? 6 : 1;
    const at = (us: number) => `translateX(${usToPx(us, pps) - half}px)`;
    let moving: Animation | undefined;
    const place = () => {
      moving?.cancel();
      moving = undefined;
      const { playing, playheadUs } = usePlayback.getState();
      const running = playing && clockAnchor().running;
      const from = running ? clockUs() : playheadUs;
      el.style.transform = at(from);
      if (running && typeof el.animate === 'function') {
        const { rate } = clockAnchor();
        const to = rate < 0 ? 0 : from + HORIZON_US;
        moving = el.animate([{ transform: at(from) }, { transform: at(to) }], {
          duration: Math.abs(to - from) / 1000 / Math.abs(rate),
          easing: 'linear',
          fill: 'forwards',
        });
      }
    };
    place();
    const offClock = onClock(place);
    const offStore = usePlayback.subscribe((s, prev) => {
      if (s.playing !== prev.playing || (!s.playing && s.playheadUs !== prev.playheadUs)) place();
    });
    return () => {
      offClock();
      offStore();
      moving?.cancel();
    };
  }, [pps, head]);
  return (
    <span
      ref={ref}
      aria-hidden
      data-playhead={head ? 'head' : 'line'}
      className={cn('tl-playhead', head && 'is-head')}
    />
  );
}
