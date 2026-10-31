import { afterEach, expect, it } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { useProject } from './project';
import { noRemovals, useProposal, clearProposalOnProjectSwitch } from './proposal';
import { clearRangeOnProjectSwitch, useRange } from './range';

const stops: (() => void)[] = [];
afterEach(() => stops.splice(0).forEach((stop) => stop()));

it("another project, or the same one opened again, starts with no time range and no proposal from the last one", () => {
  const first = { ...fixtureSnapshot(), opening: 1 };
  useProject.setState({ snapshot: first, pending: 0 });
  stops.push(clearRangeOnProjectSwitch(), clearProposalOnProjectSwitch());
  const proposal = { id: 'p', baseRevision: 3, removals: noRemovals(), tags: [] };
  const arm = () => {
    useRange.getState().select(1_000_000, 2_000_000);
    useProposal.getState().show(proposal);
  };
  arm();
  // An edit in the same opening keeps both.
  useProject.setState({ snapshot: { ...first, history: { ...first.history, revision: 4 } } });
  expect(useRange.getState().range).not.toBeNull();
  expect(useProposal.getState().proposal).toBe(proposal);
  // Another project.
  const other = fixtureSnapshot();
  other.project.id = 'other';
  useProject.setState({ snapshot: { ...other, opening: 2 } });
  expect(useRange.getState().range).toBeNull();
  expect(useProposal.getState().proposal).toBeNull();
  // The same project opened again.
  arm();
  useProject.setState({ snapshot: { ...other, opening: 3 } });
  expect(useRange.getState().range).toBeNull();
  expect(useProposal.getState().proposal).toBeNull();
});
