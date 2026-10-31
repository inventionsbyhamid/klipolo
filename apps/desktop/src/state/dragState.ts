import { create } from 'zustand';
import type { TrackKind } from '../bindings/TrackKind';

/**
 * What a timeline drag has lifted, for what draws around it: `itemKind` is the kind of layer being dragged (null: no
 * drag, or a clip or an edge), so expanded mode opens a new-track zone for it (`lanes/NewTrackZone`). Set by
 * `drag/controller` when the layer lifts, cleared when it is put down or its drop has gone in.
 */
export const useDragState = create<{ itemKind: TrackKind | null }>(() => ({ itemKind: null }));
