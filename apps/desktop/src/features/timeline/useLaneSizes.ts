import { useSyncExternalStore } from 'react';
import { MUSIC_LANE_PX, SHORT_MUSIC_LANE_PX, SHORT_SOUNDS_LANE_PX, SHORT_WINDOW, SOUNDS_LANE_PX } from './layout';

/** The Sounds and Music lanes' heights (px). */
export interface LaneSizes {
  sounds: number;
  music: number;
}

const TALL: LaneSizes = { sounds: SOUNDS_LANE_PX, music: MUSIC_LANE_PX };
const SHORT: LaneSizes = { sounds: SHORT_SOUNDS_LANE_PX, music: SHORT_MUSIC_LANE_PX };

function subscribe(changed: () => void): () => void {
  const query = window.matchMedia(SHORT_WINDOW);
  query.addEventListener('change', changed);
  return () => query.removeEventListener('change', changed);
}

const short = () => window.matchMedia(SHORT_WINDOW).matches;

/** The audio lanes' heights for the window's height now: smaller in a short window, as the CSS does. */
export function useLaneSizes(): LaneSizes {
  return useSyncExternalStore(subscribe, short) ? SHORT : TALL;
}
