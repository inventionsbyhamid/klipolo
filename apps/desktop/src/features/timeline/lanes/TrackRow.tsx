import { memo } from 'react';
import type { Asset } from '../../../bindings/Asset';
import type { Recording } from '../../../bindings/Recording';
import type { Track } from '../../../bindings/Track';
import { useLocale } from '../../../i18n/react';
import { audioBox } from './AudioLane';
import { audioRole, type AudioRole } from './audioRole';
import { linePieces, rowBox, type LineView, type Pieces } from './TrackLane';

/**
 * An audio track's layers of one kind: the track's own array when every layer is of that kind (its spans stay cached).
 */
function ofRole(track: Track, role: AudioRole) {
  const items = track.items.filter((item) => audioRole(item) === role);
  return items.length === track.items.length ? track.items : items;
}

interface TrackRowProps extends Omit<LineView, 'sources'> {
  track: Track;
  assets: Asset[];
  recordings: Recording[];
  rowPx: number;
}

/**
 * One track's layers at its row's height, while every track shows: an overlay or zoom track's as the Overlays lane
 * draws them, an audio track's sound effects and music as their lanes do. An edit to another track leaves this one's
 * object alone, so it doesn't re-render.
 */
export const TrackRow = memo(function TrackRow({ track, assets, recordings, rowPx, ...view }: TrackRowProps) {
  // Labels like "Missing media" are in the language shown: a switch re-renders the row.
  useLocale();
  const out: Pieces = { bands: [], blocks: [] };
  const line = { ...view, sources: { assets, recordings } };
  if (track.kind !== 'audio') linePieces(track.items, () => track, rowBox(rowPx), track.id, line, out);
  else
    for (const role of ['sounds', 'music'] as const) {
      const items = ofRole(track, role);
      if (items.length === 0) continue;
      const look = role === 'music' ? 'music' : 'sound';
      linePieces(items, () => track, audioBox(role, rowPx), `${track.id}:${role}`, line, out, look);
    }
  return [...out.bands, ...out.blocks];
});
