import { memo } from 'react';
import type { Asset } from '../../../bindings/Asset';
import type { Recording } from '../../../bindings/Recording';
import type { Track } from '../../../bindings/Track';
import { useLocale } from '../../../i18n/react';
import { musicRunOf } from '../../../lib/musicRuns';
import type { PressBlock, PressTrim } from '../drag/useBlockDrag';
import { audioLines, type AudioRole } from './audioRole';
import { linePieces, rowBox, type Box, type Pieces } from './TrackLane';

interface AudioLaneProps {
  /** Which sound layers it draws: the Sounds lane's effects or the Music lane's songs (`audioRole`). */
  role: AudioRole;
  /** The lane's height (px): smaller in a short window (`useLaneSizes`). */
  rowPx: number;
  /** Every track (the lane draws the audio ones): the array keeps its identity across edits that leave tracks alone. */
  tracks: Track[];
  assets: Asset[];
  recordings: Recording[];
  pps: number;
  fromUs: number;
  toUs: number;
  selectedId: string | null;
  selection: ReadonlySet<string>;
  tabStopId: string | null;
  keep: ReadonlySet<string>;
  onPick: (id: string) => void;
  onPress: PressBlock;
  onTrimPress: PressTrim;
}

/**
 * Where a lane's blocks draw in a lane `rowPx` tall: sound effects 2 px in from the Sounds lane's edges, music the
 * Music lane's full height.
 */
export function audioBox(role: AudioRole, rowPx: number): Box {
  return role === 'music' ? rowBox(rowPx, 0) : rowBox(rowPx, 2);
}

/**
 * The Sounds or the Music lane: every audio track's layers of that kind in one line, music being the
 * sound layers that duck under speech (`audioRole`). A lane's bands are drawn before its blocks, so a band never
 * covers a sound.
 */
export const AudioLane = memo(function AudioLane({
  role,
  rowPx,
  tracks,
  assets,
  recordings,
  ...view
}: AudioLaneProps) {
  // Labels like "Missing media" are in the language shown: a switch re-renders the lane.
  useLocale();
  const out: Pieces = { bands: [], blocks: [] };
  const look = role === 'music' ? 'music' : 'sound';
  const box = audioBox(role, rowPx);
  // A song laid in pieces is one: selecting a piece highlights the others with it.
  const run = role === 'music' && view.selectedId ? musicRunOf(tracks, view.selectedId) : null;
  const runIds = run && run.length > 1 ? new Set(run.map((piece) => piece.id)) : undefined;
  const line = { ...view, runIds, sources: { assets, recordings } };
  for (const { track, items } of audioLines(tracks)[role].lines)
    linePieces(items, () => track, box, `${role}:${track.id}`, line, out, look);
  return [...out.bands, ...out.blocks];
});
