import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import type { TimeRange } from '../../../state/range';
import { mainIndex } from './index';

/**
 * The edit that takes `range` out of the video: one `deleteRanges` over the main track's part of it, so the gap
 * closes and attached layers follow (one undo step). Empty when the range holds no footage (it is past the clips).
 */
export function deleteRangeOps(project: Project, range: TimeRange): Op[] {
  const { mainEndUs } = mainIndex(project.timeline.main);
  const start = Math.max(0, Math.round(range.startUs));
  const end = Math.min(mainEndUs, Math.round(range.endUs));
  return end > start ? [{ type: 'deleteRanges', ranges: [{ start, end }] }] : [];
}
