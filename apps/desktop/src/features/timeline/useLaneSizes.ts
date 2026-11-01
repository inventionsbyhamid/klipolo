import { useSyncExternalStore } from 'react';
import {
  MUSIC_LANE_PX,
  SHORT_MUSIC_LANE_PX,
  SHORT_SOUNDS_LANE_PX,
  SHORT_SOURCE_LANE_PX,
  SHORT_WINDOW,
  SOUNDS_LANE_PX,
  SOURCE_LANE_PX,
} from './layout';

/** The heights (px) of the Sounds and Music lanes and of each recorded sound's lane under Video. */
export interface LaneSizes {
  sounds: number;
  music: number;
  source: number;
}

const TALL: LaneSizes = { sounds: SOUNDS_LANE_PX, music: MUSIC_LANE_PX, source: SOURCE_LANE_PX };
const SHORT: LaneSizes = { sounds: SHORT_SOUNDS_LANE_PX, music: SHORT_MUSIC_LANE_PX, source: SHORT_SOURCE_LANE_PX };

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
