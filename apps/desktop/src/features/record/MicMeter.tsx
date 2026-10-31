import type { CSSProperties } from 'react';

/** The meter's share of a peak: empty at -60 dBFS or below, full at 0 dBFS. */
export function meterShare(dbfs: number | null): number {
  if (dbfs === null || !Number.isFinite(dbfs)) return 0;
  return Math.min(1, Math.max(0, (dbfs + 60) / 60));
}

/**
 * The microphone's level: a 6 px pill track, its paper fill following the peaks, under the Microphone
 * select while choosing and under the countdown's sentence. One shows at a time: the choices are hidden in the
 * countdown.
 */
export function MicMeter({ dbfs }: { dbfs: number | null }) {
  const share = meterShare(dbfs);
  return (
    <div
      className="record-meter"
      role="meter"
      aria-label="Microphone level"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(share * 100)}
      style={{ '--p': `${Math.round(share * 1000) / 10}%` } as CSSProperties}
    >
      <i />
    </div>
  );
}
