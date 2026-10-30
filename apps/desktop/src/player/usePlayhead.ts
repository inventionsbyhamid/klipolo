import { useEffect, useState } from 'react';
import { playheadAt, useSession } from './session';

/** The playhead in µs, redrawn every animation frame while playing. */
export function usePlayhead(): number {
  const playing = useSession((s) => s.playing);
  // Re-render when the engine moves the anchor (a seek, a report, a stop).
  useSession((s) => s.anchorUs + s.anchorMs);
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!playing) return;
    let raf = requestAnimationFrame(function loop() {
      setTick((t) => t + 1);
      raf = requestAnimationFrame(loop);
    });
    return () => cancelAnimationFrame(raf);
  }, [playing]);
  return playheadAt(Date.now());
}
