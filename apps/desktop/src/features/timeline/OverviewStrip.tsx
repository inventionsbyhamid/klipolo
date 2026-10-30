import { useLingui } from '@lingui/react/macro';
import { memo, useEffect, useRef, type CSSProperties, type PointerEvent, type RefObject } from 'react';
import type { Clip } from '../../bindings/Clip';
import { useProposal } from '../../state/proposal';
import { HEADER_PX } from './layout';
import { mainIndex } from './model/index';

/** Segments and ticks closer than this share of the strip draw as one, so an hour of cuts is a few dozen elements. */
const MIN_SHARE = 0.004;

/** `[from, to)` shares of the strip (0 to 1) of each run of clips, nearby ones merged. */
export function overviewSegments(main: Clip[], durationUs: number): [number, number][] {
  if (durationUs <= 0) return [];
  const { starts, ends } = mainIndex(main);
  const out: [number, number][] = [];
  for (let i = 0; i < main.length; i++) {
    const from = starts[i] / durationUs;
    const to = ends[i] / durationUs;
    const last = out.at(-1);
    if (last && to - last[0] < MIN_SHARE) last[1] = to;
    else out.push([from, to]);
  }
  return out;
}

/** Each proposed cut's place (a share of the strip), nearby ones counted once. */
export function overviewTicks(starts: readonly number[], durationUs: number): number[] {
  const out: number[] = [];
  for (const us of starts) {
    const at = us / durationUs;
    if (out.length === 0 || at - out[out.length - 1] >= MIN_SHARE) out.push(at);
  }
  return out;
}

interface OverviewStripProps {
  main: Clip[];
  /** The video's length (µs) and the zoom: the strip is the whole video, its outline the lanes' view. */
  durationUs: number;
  pps: number;
  /** The lanes' scroller, whose scroll the outline follows (outside React) and which a drag of it scrolls. */
  scrollRef: RefObject<HTMLElement | null>;
}

/**
 * A 6 px pill of the whole project over the ruler: the clips in deep blue, a red tick at each proposed
 * cut, and the part the lanes show outlined in paper. Drag the outline (or press elsewhere on the strip) to scroll the
 * lanes there. Hidden when the whole project fits. The outline moves with every scroll by its style, never a render.
 */
export const OverviewStrip = memo(function OverviewStrip({ main, durationUs, pps, scrollRef }: OverviewStripProps) {
  const { t } = useLingui();
  const proposal = useProposal((s) => s.proposal);
  const strip = useRef<HTMLDivElement>(null);
  const outline = useRef<HTMLSpanElement>(null);
  const totalPx = (durationUs / 1e6) * pps;

  // After the commit, not in a layout effect: the scroller comes after the strip, so its ref isn't attached yet while
  // the strip's layout effects run. Until placed, the strip stays hidden (no `data-fits`).
  useEffect(() => {
    const scroller = scrollRef.current;
    const box = strip.current;
    const el = outline.current;
    if (!scroller || !box || !el || durationUs <= 0) return;
    const place = () => {
      const view = Math.max(1, scroller.clientWidth - HEADER_PX);
      const fits = totalPx <= view;
      // Its row keeps its height either way, so the lanes never jump.
      box.dataset.fits = String(fits);
      if (fits) return;
      const from = scroller.scrollLeft / totalPx;
      const share = Math.min(1, view / totalPx);
      el.style.left = `${Math.min(1, from) * 100}%`;
      el.style.width = `${share * 100}%`;
    };
    place();
    scroller.addEventListener('scroll', place, { passive: true });
    const sizes = new ResizeObserver(place);
    sizes.observe(scroller);
    return () => {
      scroller.removeEventListener('scroll', place);
      sizes.disconnect();
    };
  }, [scrollRef, durationUs, totalPx]);

  // Nothing yet: the empty pill, so the row keeps its place and the lanes don't move when a clip comes in.
  if (durationUs <= 0)
    return <div data-overview data-fits="false" className="tl-overview" style={{ marginLeft: HEADER_PX }} />;

  /** Scrolls the lanes so the outline's middle is at `clientX`, and follows the pointer while it is held. */
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    const scroller = scrollRef.current;
    if (e.button !== 0 || !scroller) return;
    e.preventDefault();
    const bar = e.currentTarget;
    bar.setPointerCapture?.(e.pointerId);
    const rect = bar.getBoundingClientRect();
    const view = Math.max(1, scroller.clientWidth - HEADER_PX);
    const onOutline = e.target === outline.current;
    // Held by the outline: it keeps where it was grabbed; elsewhere it centres on the pointer.
    const grab = onOutline
      ? e.clientX - outline.current!.getBoundingClientRect().left
      : ((view / totalPx) * rect.width) / 2;
    const scrollTo = (clientX: number) => {
      const share = (clientX - grab - rect.left) / Math.max(1, rect.width);
      scroller.scrollLeft = Math.max(0, share * totalPx);
    };
    scrollTo(e.clientX);
    const move = (ev: globalThis.PointerEvent) => scrollTo(ev.clientX);
    const end = () => {
      bar.removeEventListener('pointermove', move);
      bar.removeEventListener('pointerup', end);
      bar.removeEventListener('pointercancel', end);
    };
    bar.addEventListener('pointermove', move);
    bar.addEventListener('pointerup', end);
    bar.addEventListener('pointercancel', end);
  };

  const ticks = proposal ? overviewTicks(proposal.removals.ranges.map((r) => r.start), durationUs) : [];
  return (
    <div
      ref={strip}
      data-overview
      className="tl-overview"
      style={{ marginLeft: HEADER_PX }}
      onPointerDown={onPointerDown}
      title={t({ id: 'timeline.overview.tip', message: 'Drag to scroll the timeline' })}
    >
      {overviewSegments(main, durationUs).map(([from, to]) => (
        <i
          key={from}
          className="ov-seg"
          style={{ '--x': `${from * 100}%`, '--w': `${(to - from) * 100}%` } as CSSProperties}
        />
      ))}
      {ticks.map((at) => (
        <i key={at} data-overview-cut className="ov-tick" style={{ '--x': `${at * 100}%` } as CSSProperties} />
      ))}
      <span ref={outline} className="ov-view" />
    </div>
  );
});
