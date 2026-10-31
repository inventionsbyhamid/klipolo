import { create } from 'zustand';
import { speech } from '../bridge/speech';

interface SpeechStore {
  /** Moves on when what was said changes (`speech-changed`): caption pages are asked for again. */
  pagesEpoch: number;
}

/** What was said, as the caption views follow it: a pages epoch `speech-changed` moves on. */
export const useSpeech = create<SpeechStore>(() => ({ pagesEpoch: 0 }));

let started = false;

/** Follows `speech-changed` from the first view that shows caption pages. */
export function startSpeechFeed() {
  if (started) return;
  started = true;
  speech
    .onChanged(() => useSpeech.setState((s) => ({ pagesEpoch: s.pagesEpoch + 1 })))
    .catch(() => undefined);
}
