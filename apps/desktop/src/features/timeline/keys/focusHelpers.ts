import type { Project } from '../../../bindings/Project';
import { findItem } from '../../../lib/timeline';
import { placeOf, type Lane } from './laneOrder';

/** What `useRovingFocus` asks about focus and blocks. */

/** The timeline block an event's target is in. */
export const blockOf = (target: EventTarget | null) =>
  target instanceof Element ? target.closest<HTMLElement>('[data-block-id]') : null;

/** Nothing has focus (it dropped to the page). */
export const pageFocused = () => document.activeElement === null || document.activeElement === document.body;

/** Whether block `id` is still in the project (a clip or a layer). */
export const inProject = (project: Project, id: string) =>
  findItem(project, id) !== null || project.timeline.main.some((c) => c.id === id);

/** Where block `id`'s lane neighbour is in `after`, for a block that's gone: the next one, else the one before. */
export function neighbour(before: Lane[], after: Lane[], id: string): string | null {
  const at = placeOf(before, id);
  if (!at) return null;
  const lane = after.find((l) => l.id === before[at[0]].id);
  if (!lane || lane.ids.length === 0) return null;
  return lane.ids[Math.min(at[1], lane.ids.length - 1)];
}
