import { memo, type CSSProperties } from 'react';
import { cn } from '../../design/cn';
import { formatTime } from '../../player/time';
import { useProposal, type Proposal } from '../../state/proposal';
import { RULER_PX } from './layout';
import { inWindow, mainGhosts } from './ProposalOverlay';
import { usToPx } from './scale';

/** Ranges drawn this soon after their proposal arrived draw in; later ones (a scroll) just appear. */
const DRAW_IN_MS = 1000;
/** Tags closer than this (px, middle to middle) would overlap: zoomed out, only the first of them shows. */
const TAG_GAP_PX = 44;

/** Each removed range's tag ("um", "pause"): the proposal's tag over the most of it, built once per proposal. */
const tagsOf = new WeakMap<Proposal, string[]>();
function rangeTags(proposal: Proposal): string[] {
  let tags = tagsOf.get(proposal);
  if (!tags) {
    tags = proposal.removals.ranges.map((r) => {
      let best = '';
      let most = 0;
      for (const tag of proposal.tags) {
        const overlap = Math.min(r.end, tag.endUs) - Math.max(r.start, tag.startUs);
        if (overlap > most) [best, most] = [tag.label, overlap];
      }
      return best;
    });
    tagsOf.set(proposal, tags);
  }
  return tags;
}

interface ProposalRangesProps {
  pps: number;
  fromUs: number;
  toUs: number;
}

/**
 * The footage a waiting proposal cuts, from the ruler's bottom through every lane: 2 px red edges, a
 * hatched red wash, at least 4 px wide, with its tag at the top ("um", "pause"; zoomed out, a tag that would overlap
 * the one before it is left off). Hovering a tag says the exact range.
 * When the proposal arrives its ranges draw in left to right (360 ms, 40 ms apart; none with reduced motion). Only
 * the lanes' window's ranges are drawn, found by binary search; the body lets clicks through to the blocks.
 */
export const ProposalRanges = memo(function ProposalRanges({ pps, fromUs, toUs }: ProposalRangesProps) {
  const proposal = useProposal((s) => s.proposal);
  const arrivedAt = useProposal((s) => s.arrivedAt);
  if (!proposal) return null;
  const ghosts = mainGhosts(proposal);
  const tags = rangeTags(proposal);
  const fresh = performance.now() - arrivedAt < DRAW_IN_MS;
  let lastTagPx = -Infinity;
  return inWindow(ghosts, fromUs, toUs).map((i, order) => {
    // Clipped to the window, so a long cut zoomed in is never a huge element; an edge shows only where it is.
    const from = Math.max(ghosts.starts[i], fromUs);
    const to = Math.min(ghosts.ends[i], toUs);
    const left = usToPx(from, pps);
    const width = Math.max(4, usToPx(to, pps) - left);
    const middle = left + width / 2;
    const tag = tags[i] && middle - lastTagPx >= TAG_GAP_PX ? tags[i] : '';
    if (tag) lastTagPx = middle;
    const range = `${formatTime(ghosts.starts[i])} to ${formatTime(ghosts.ends[i])}`;
    return (
      <div
        key={`cut:${i}`}
        data-cut={i}
        data-ghost="footage"
        aria-hidden
        className={cn(
          'tl-cut',
          fresh && 'is-new',
          from > ghosts.starts[i] && 'no-start',
          to < ghosts.ends[i] && 'no-end',
        )}
        style={{ left, width, top: RULER_PX + 2, '--i': order } as CSSProperties}
      >
        {tag && (
          <b className="tl-cut-tag" title={range}>
            {tag}
          </b>
        )}
      </div>
    );
  });
});
