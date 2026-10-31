import { mainIndex } from '../features/timeline/model/index';
import { useProject } from '../state/project';
import { noRemovals, useProposal, type Proposal } from '../state/proposal';

const S = 1_000_000;

/**
 * A proposal like chat's "cut the ums and long pauses", over the open project's clips: an "um" or a pause every few
 * seconds. Development only, until chat (Plan 4) proposes real cuts: it lets the timeline's proposed cuts be seen.
 */
export function sampleProposal(): Proposal | null {
  const snapshot = useProject.getState().snapshot;
  if (!snapshot) return null;
  const { mainEndUs } = mainIndex(snapshot.project.timeline.main);
  const ranges = [];
  const tags = [];
  for (let at = 3 * S, k = 0; at + S < mainEndUs; at += 6.5 * S, k++) {
    const pause = k % 3 === 1;
    const end = at + (pause ? 1.4 * S : 0.4 * S);
    ranges.push({ start: at, end });
    tags.push({ startUs: at, endUs: end, label: pause ? 'pause' : 'um' });
  }
  return {
    id: `sample-${snapshot.history.revision}`,
    baseRevision: snapshot.history.revision,
    removals: { ...noRemovals(), ranges },
    tags,
  };
}

/** Control Shift P shows the sample proposal, or takes it away (development builds only). */
export function listenForSampleProposal(): () => void {
  const onKey = (e: KeyboardEvent) => {
    if (!e.ctrlKey || !e.shiftKey || e.code !== 'KeyP') return;
    e.preventDefault();
    const { proposal, show } = useProposal.getState();
    show(proposal ? null : sampleProposal());
  };
  window.addEventListener('keydown', onKey);
  return () => window.removeEventListener('keydown', onKey);
}
