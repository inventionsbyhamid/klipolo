/** A time for people: `m:ss.d` under an hour, `h:mm:ss` from an hour. Rounds down to the tenth shown. */
export function formatTime(us: number): string {
  const tenths = Math.floor(Math.max(us, 0) / 100_000);
  const totalSeconds = Math.floor(tenths / 10);
  const s = totalSeconds % 60;
  const m = Math.floor(totalSeconds / 60) % 60;
  const h = Math.floor(totalSeconds / 3600);
  const ss = String(s).padStart(2, '0');
  if (h > 0) return `${h}:${String(m).padStart(2, '0')}:${ss}`;
  return `${m}:${ss}.${tenths % 10}`;
}
