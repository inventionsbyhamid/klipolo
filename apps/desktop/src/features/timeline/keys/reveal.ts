import { gutterPx } from '../layout';
import { usToPx } from '../scale';
import { placeOf, type Lane } from './laneOrder';

/** Scrolls the lanes to block `id` when it's out of view: its start a third of the way in (and its row into view). */
export function reveal(scroller: HTMLElement, lanes: Lane[], id: string, pps: number) {
  const at = placeOf(lanes, id);
  const view = scroller.clientWidth - gutterPx();
  if (!at || view <= 0) return;
  const start = usToPx(lanes[at[0]].starts[at[1]], pps);
  const end = usToPx(lanes[at[0]].ends[at[1]], pps);
  if (end <= scroller.scrollLeft || start >= scroller.scrollLeft + view)
    scroller.scrollLeft = Math.max(0, start - view / 3);
}

/** The block's row into view under the sticky ruler, when every track shows and they scroll. */
export function revealRow(scroller: HTMLElement, el: HTMLElement) {
  const box = scroller.getBoundingClientRect();
  if (box.height <= 0) return;
  const r = el.getBoundingClientRect();
  const top = scroller.querySelector('[data-testid="ruler"]')?.getBoundingClientRect().bottom ?? box.top;
  if (r.top < top) scroller.scrollTop -= top - r.top;
  else if (r.bottom > box.bottom) scroller.scrollTop += r.bottom - box.bottom;
}
