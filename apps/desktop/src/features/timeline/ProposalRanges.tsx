import { memo, type CSSProperties } from 'react';
import { cn } from '../../design/cn';
import { useProposal, type Proposal } from '../../state/proposal';
import { RULER_PX } from './layout';
import { inWindow, mainGhosts } from './ProposalOverlay';
import { usToPx } from './scale';

/** Ranges drawn this soon after their proposal arrived draw in; later ones (a scroll) just appear. */
const DRAW_IN_MS = 1000;
/** A tag's width as drawn (11 px bold text, 5 px padding each side), estimated, and the most it takes (app.css). */
const TAG_CHAR_PX = 6.6;
const TAG_PAD_PX = 10;
const TAG_MAX_PX = 160;
/** The space kept between two shown tags (px). */
const TAG_GAP_PX = 4;

function tagWidth(label: string): number {
  return Math.min(TAG_MAX_PX, label.length * TAG_CHAR_PX + TAG_PAD_PX);
}

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
 * hatched red wash, at least 4 px wide, with its tag at the top ("um", "pause"; at most 160 px with an ellipsis, and
 * left off where it would overlap the one before it). Neither the range nor its tag takes the pointer.
 * When the proposal arrives its ranges draw in left to right (360 ms, 40 ms apart; none with reduced motion). Only
 * the lanes' window's ranges are drawn, found by binary search; the body lets clicks through to the blocks.
 */
export const ProposalRanges = memo(function ProposalRanges({ pps, fromUs, toUs }: ProposalRangesProps) {
  const proposal = useProposal((s) => s.proposal);
  const arrivedAt = useProposal((s) => s.arrivedAt);
  const hovered = useProposal((s) => s.hovered);
  if (!proposal) return null;
  const ghosts = mainGhosts(proposal);
  const tags = rangeTags(proposal);
  const fresh = performance.now() - arrivedAt < DRAW_IN_MS;
  let lastTagEnd = -Infinity;
  return inWindow(ghosts, fromUs, toUs).map((i, order) => {
    // Clipped to the window, so a long cut zoomed in is never a huge element; an edge shows only where it is.
    const from = Math.max(ghosts.starts[i], fromUs);
    const to = Math.min(ghosts.ends[i], toUs);
    const left = usToPx(from, pps);
    const width = Math.max(4, usToPx(to, pps) - left);
    // Centred over its range; left off when it would run into the last tag shown (zoomed out, many are close).
    const half = tags[i] ? tagWidth(tags[i]) / 2 : 0;
    const middle = left + width / 2;
    const tag = tags[i] && middle - half >= lastTagEnd + TAG_GAP_PX ? tags[i] : '';
    if (tag) lastTagEnd = middle + half;
    return (
      <div
        key={`cut:${i}`}
        data-cut={i}
        data-ghost="footage"
        aria-hidden
        className={cn(
          'tl-cut',
          fresh && 'is-new',
          i === hovered && 'is-hovered',
          from > ghosts.starts[i] && 'no-start',
          to < ghosts.ends[i] && 'no-end',
        )}
        style={{ left, width, top: RULER_PX + 2, '--i': order } as CSSProperties}
      >
        {tag && <b className="tl-cut-tag">{tag}</b>}
      </div>
    );
  });
});
