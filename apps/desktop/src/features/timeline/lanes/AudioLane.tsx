import { memo } from 'react';
import type { Asset } from '../../../bindings/Asset';
import type { Recording } from '../../../bindings/Recording';
import type { Track } from '../../../bindings/Track';
import { useLocale } from '../../../i18n/react';
import type { PressBlock, PressTrim } from '../drag/useBlockDrag';
import { MUSIC_LANE_PX, SOUNDS_LANE_PX } from '../layout';
import { audioLines, type AudioRole } from './audioRole';
import { linePieces, rowBox, type Box, type Pieces } from './TrackLane';

interface AudioLaneProps {
  /** Which sound layers it draws: the Sounds lane's effects or the Music lane's songs (`audioRole`). */
  role: AudioRole;
  /** Every track (the lane draws the audio ones): the array keeps its identity across edits that leave tracks alone. */
  tracks: Track[];
  assets: Asset[];
  recordings: Recording[];
  pps: number;
  fromUs: number;
  toUs: number;
  selectedId: string | null;
  tabStopId: string | null;
  keep: ReadonlySet<string>;
  onPick: (id: string) => void;
  onPress: PressBlock;
  onTrimPress: PressTrim;
}

/**
 * Where a lane's blocks draw: sound effects 2 px in from the Sounds lane's edges, music the Music lane's full height.
 */
export function audioBox(role: AudioRole): Box {
  return role === 'music' ? rowBox(MUSIC_LANE_PX, 0) : rowBox(SOUNDS_LANE_PX, 2);
}

/**
 * The Sounds or the Music lane: every audio track's layers of that kind in one line, music being the
 * sound layers that duck under speech (`audioRole`). A lane's bands are drawn before its blocks, so a band never
 * covers a sound.
 */
export const AudioLane = memo(function AudioLane({ role, tracks, assets, recordings, ...view }: AudioLaneProps) {
  // Labels like "Missing media" are in the language shown: a switch re-renders the lane.
  useLocale();
  const out: Pieces = { bands: [], blocks: [] };
  const look = role === 'music' ? 'music' : 'sound';
  const box = audioBox(role);
  const line = { ...view, sources: { assets, recordings } };
  for (const { track, items } of audioLines(tracks)[role].lines)
    linePieces(items, () => track, box, `${role}:${track.id}`, line, out, look);
  return [...out.bands, ...out.blocks];
});
