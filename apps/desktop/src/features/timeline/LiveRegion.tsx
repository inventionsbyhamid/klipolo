import { useAnnouncer } from './announce';

/**
 * Says each timeline edit to screen readers. The region stays mounted for the timeline's lifetime (a live region
 * inserted with its text often isn't read); each message is a new span in it, so the same words twice are heard twice.
 */
export function LiveRegion() {
  const message = useAnnouncer((s) => s.message);
  const seq = useAnnouncer((s) => s.seq);
  return (
    <div role="status" aria-live="polite" className="sr-only">
      <span key={seq}>{message}</span>
    </div>
  );
}
