import { describe, expect, it } from 'vitest';
import type { Project } from '../bindings/Project';
import cases from './transition-parity.json';
import {
  DEFAULT_TRANSITION_US,
  defaultTransitionUs,
  fittedTransitionsUs,
  maxTransitionUs,
  splitDuration,
  transitionWindows,
  type TransitionWindow,
} from './transitions';
import { clipStarts } from './timeline';

interface Case {
  project: Project;
  max: number[];
  windows: TransitionWindow[];
  /** Each clip's transition length once Rust's `normalize` has refitted the project (0: none). */
  fitted: number[];
}
const all = cases as unknown as Case[];

describe('transitions (pinned to Rust by transition-parity.json)', () => {
  it('offers the same longest transition at every cut', () => {
    for (const { project, max } of all) {
      expect(project.timeline.main.map((_, i) => maxTransitionUs(project, i))).toEqual(max);
    }
  });

  it('leaves each neighbour its share of a clip between two transitions', () => {
    // "b" (2 s) gives its first 0.500001 s to the transition from "a" and its last 1 s to its own.
    const shared = all[4].project;
    expect(shared.timeline.main.map((_, i) => maxTransitionUs(shared, i))).toEqual([2_000_000, 2_999_999, 0]);
  });

  it('places every window where Rust does', () => {
    for (const { project, windows } of all) {
      const main = project.timeline.main;
      expect(transitionWindows(main, clipStarts(main))).toEqual(windows);
    }
  });

  it("refits an edit's transitions as Rust does: moved, shortened, dropped, or out of footage after a trim", () => {
    for (const { project, fitted } of all) {
      expect(fittedTransitionsUs(project)).toEqual(fitted);
    }
    // The edits as previewed: "a" moved last (dropped), "b" moved last (kept), "a"'s end trimmed out to its file's end
    // (dropped), and a clip with 1 s of spare footage moved after "a" (shortened).
    expect(all.slice(-4).map((c) => c.fitted)).toEqual([
      [0, 0, 0],
      [3_000_000, 0, 0],
      [0, 0],
      [2_000_001, 0, 0],
    ]);
  });

  it('splits a length around the cut with the odd microsecond after it', () => {
    expect(splitDuration(500_001)).toEqual([250_000, 250_001]);
  });

  it('defaults to half a second on the frame grid, or the longest that fits', () => {
    const [, withHandles] = cases as unknown as { project: Project }[];
    expect(defaultTransitionUs(withHandles.project, 0, 30)).toBe(DEFAULT_TRANSITION_US);
    const [bare] = cases as unknown as { project: Project }[];
    expect(defaultTransitionUs(bare.project, 0, 30)).toBe(0);
  });

  it('places windows centred on their cuts', () => {
    const [, , three] = cases as unknown as { project: Project }[];
    const main = three.project.timeline.main;
    expect(transitionWindows(main, clipStarts(main))).toEqual([
      { clipIndex: 1, startUs: 25_000_000, cutUs: 26_000_000, endUs: 27_000_000 },
    ]);
  });
});
