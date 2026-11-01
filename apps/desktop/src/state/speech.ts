import { create } from 'zustand';
import { speech } from '../bridge/speech';

interface SpeechStore {
  /** Moves on when what was said changes (`speech-changed`): caption pages are asked for again. */
  pagesEpoch: number;
  /**
   * The files whose words the last change heard, timed again or took away, by asset id: an open chat proposal whose
   * cuts stand on one of them is dropped.
   */
  wordsChanged: string[];
}

/** What was said, as the caption views follow it: a pages epoch `speech-changed` moves on. */
export const useSpeech = create<SpeechStore>(() => ({ pagesEpoch: 0, wordsChanged: [] }));

let started = false;

/** Follows `speech-changed` from the first view that shows caption pages. */
export function startSpeechFeed() {
  if (started) return;
  started = true;
  speech
    .onChanged((files) => useSpeech.setState((s) => ({ pagesEpoch: s.pagesEpoch + 1, wordsChanged: files })))
    .catch(() => undefined);
}
