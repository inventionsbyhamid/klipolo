import { create } from 'zustand';
import type { Removals } from '../bindings/Removals';
import { useProject } from './project';

/** A short reason shown on a proposed cut ("um", "pause"), over the time it covers. */
export interface ProposalTag {
  startUs: number;
  endUs: number;
  label: string;
}

/**
 * A change chat proposes and the person has not applied yet: what it would take away (`removals`, on the timeline as
 * it is now, the project at `baseRevision`) and a tag for each cut.
 */
export interface Proposal {
  id: string;
  baseRevision: number;
  removals: Removals;
  tags: ProposalTag[];
}

interface ProposalStore {
  proposal: Proposal | null;
  /** When it arrived (`performance.now()`): its ranges draw in only then, not again on a scroll. */
  arrivedAt: number;
  show: (proposal: Proposal | null) => void;
  /** The range the person points at in the proposal card's list (its index in `removals.ranges`), lit on the timeline. */
  hovered: number | null;
  hover: (index: number | null) => void;
}

/**
 * The proposal waiting for the person, which the timeline draws as red ranges and the chat as a card. Chat sets it;
 * Apply, Discard or a hand edit clear it.
 */
export const useProposal = create<ProposalStore>((set) => ({
  proposal: null,
  arrivedAt: 0,
  show: (proposal) => set({ proposal, arrivedAt: proposal ? performance.now() : 0, hovered: null }),
  hovered: null,
  hover: (hovered) => set({ hovered }),
}));

/** Nothing taken away (a start for a proposal's `removals`). */
export function noRemovals(): Removals {
  return {
    ranges: [],
    clips: [],
    items: [],
    shortened: [],
    tracks: [],
    effects: [],
    transitions: [],
    assets: [],
    luts: [],
    recordings: [],
    markers: [],
    sounds: [],
    formats: [],
    moved: [],
  };
}

/**
 * Drops the waiting proposal when another project opens, or the same one opens again: its cuts are times on the
 * project it was made for. Returns the stop.
 */
export function clearProposalOnProjectSwitch(): () => void {
  return useProject.subscribe((s, prev) => {
    if (s.snapshot?.project.id !== prev.snapshot?.project.id || s.snapshot?.opening !== prev.snapshot?.opening)
      useProposal.getState().show(null);
  });
}
