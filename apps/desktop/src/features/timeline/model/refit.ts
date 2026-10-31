import { plural, t } from '@lingui/core/macro';
import type { Clip } from '../../../bindings/Clip';
import type { Project } from '../../../bindings/Project';
import { fittedTransitionsUs } from '../../../lib/transitions';

/** How many transitions an edit removes (they no longer fit) and shortens, once the model refits them. */
export interface Refit {
  removed: number;
  shortened: number;
}

const NONE: Refit = { removed: 0, shortened: 0 };

/** Counts, over `clips[first, end)`, the transitions `main` would refit shorter or away as `next`. */
function compare(project: Project, next: Clip[], first: number, end: number): Refit {
  const fitted = fittedTransitionsUs({ ...project, timeline: { ...project.timeline, main: next } });
  const out = { removed: 0, shortened: 0 };
  for (let i = Math.max(0, first); i < Math.min(end, next.length); i++) {
    const want = next[i].transitionOut?.durationUs;
    if (want === undefined) continue;
    if (fitted[i] === 0) out.removed++;
    else if (fitted[i] < want) out.shortened++;
  }
  return out;
}

/** What moving clip `from` to `to` does to transitions: a clip moved to the end loses its own, and so on. */
export function reorderRefit(project: Project, from: number, to: number): Refit {
  const main = project.timeline.main;
  if (from === to || !main.some((c) => c.transitionOut)) return NONE;
  const moved = main.slice();
  moved.splice(to, 0, ...moved.splice(from, 1));
  return compare(project, moved, 0, moved.length);
}

/**
 * What trimming clip `i` to `trimmed` does to the transitions into and out of it (an end trimmed out to its file's end
 * leaves its transition no footage). Only those two can change: the clips around them are refitted as a window
 * starting two clips earlier, so the transition into `i` sees what comes before it.
 */
export function trimRefit(project: Project, i: number, trimmed: Clip): Refit {
  const first = Math.max(0, i - 2);
  const window = project.timeline.main.slice(first, i + 2);
  if (!window.some((c) => c.transitionOut)) return NONE;
  window[i - first] = trimmed;
  return compare(project, window, i - 1 - first, i + 1 - first);
}

/**
 * What an edit does to transitions, as a sentence of its own ("A transition is removed.", "2 transitions are removed
 * and one is shortened."); empty when nothing changes. Edits say it after what they did, as a second sentence.
 */
export function refitNote({ removed, shortened }: Refit): string {
  if (removed && shortened)
    return t({
      id: 'timeline.refit.both',
      message: `${plural(removed, { 1: 'A transition is removed', other: '# transitions are removed' })} and ${plural(shortened, { 1: 'one is shortened', other: '# are shortened' })}.`,
      comment: 'What an edit does to transitions, said after what it did',
    });
  if (removed)
    return t({
      id: 'timeline.refit.removes',
      message: plural(removed, { 1: 'A transition is removed.', other: '# transitions are removed.' }),
      comment: 'What an edit does to transitions, said after what it did',
    });
  if (shortened)
    return t({
      id: 'timeline.refit.shortens',
      message: plural(shortened, { 1: 'A transition is shortened.', other: '# transitions are shortened.' }),
      comment: 'What an edit does to transitions, said after what it did',
    });
  return '';
}
