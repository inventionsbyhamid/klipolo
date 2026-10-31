import { create } from 'zustand';

interface Announcer {
  message: string;
  seq: number;
}

/** What the timeline's live region says next (screen readers hear each edit). */
export const useAnnouncer = create<Announcer>(() => ({ message: '', seq: 0 }));

export function announce(message: string) {
  if (message) useAnnouncer.setState((s) => ({ message, seq: s.seq + 1 }));
}
