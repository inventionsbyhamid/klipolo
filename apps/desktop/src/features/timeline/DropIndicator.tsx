import { useLayoutEffect, useRef } from 'react';
import { cn } from '../../design/cn';
import { useProject } from '../../state/project';
import { useDropIndicator } from './dropTarget';
import { layerStartUs, type DropTarget } from './model/dropOps';
import { mainIndex } from './model/index';
import { usToPx } from './scale';

/** A ghost whose file's length isn't known yet (files still in Finder). */
const UNKNOWN_PX = 48;
/** A ghost's inset from its row's top and bottom. */
const INSET_PX = 4;

const escapeId = (id: string) => (typeof CSS !== 'undefined' && CSS.escape ? CSS.escape(id) : id);

/** The row a target is in, among the lanes in `lanes` (the timeline's scrolled content). */
function rowOf(lanes: Element, target: DropTarget): HTMLElement | null {
  if (target.lane === 'main') return lanes.querySelector('[data-lane="main"]');
  if (target.newTrack) return lanes.querySelector('[data-new-track]');
  if (target.trackId !== null) return lanes.querySelector(`[data-track-id="${escapeId(target.trackId)}"]`);
  return lanes.querySelector(target.lane === 'sound' ? '[data-lane="sounds"]' : '[data-lane="layers"]');
}

/**
 * Where dragged media would land (`useDropIndicator`): a 2 px line at a cut of the main track, or a dashed ghost as
 * long as the media at its time on a layer's row. It sits in the lanes' overlay (x 0 is the video's start) and is
 * placed by transform; it re-renders only when the target changes, never on the timeline's own renders.
 */
export function DropIndicator({ pps }: { pps: number }) {
  const target = useDropIndicator((s) => s.target);
  const lengthUs = useDropIndicator((s) => s.lengthUs);
  const ref = useRef<HTMLDivElement>(null);
  const line = target?.lane === 'main';
  useLayoutEffect(() => {
    const el = ref.current;
    const lanes = el?.parentElement?.parentElement;
    const row = target && lanes ? rowOf(lanes, target) : null;
    if (!el || !target || !row) {
      if (el) el.style.display = 'none';
      return;
    }
    const project = useProject.getState().snapshot?.project;
    if (!project) return;
    const { starts, mainEndUs } = mainIndex(project.timeline.main);
    // Where `dropOps` will start it: a cut, or a layer's edge, exactly; else the frame grid.
    const atUs =
      target.lane === 'main'
        ? target.index < starts.length
          ? starts[target.index]
          : mainEndUs
        : layerStartUs(project, target.atUs, project.settings.fps);
    const x = usToPx(atUs, pps);
    const [top, height] = line
      ? [row.offsetTop, row.offsetHeight]
      : [row.offsetTop + INSET_PX, Math.max(0, row.offsetHeight - 2 * INSET_PX)];
    el.style.display = 'block';
    el.style.height = `${height}px`;
    el.style.width = line ? '' : `${lengthUs === null ? UNKNOWN_PX : Math.max(4, usToPx(lengthUs, pps))}px`;
    el.style.transform = `translate(${line ? x - 1 : x}px, ${top}px)`;
  }, [target, lengthUs, pps, line]);
  return (
    <div
      ref={ref}
      aria-hidden
      data-drop-indicator={target === null ? undefined : target.lane}
      className={cn(
        'pointer-events-none absolute top-0 left-0 hidden',
        line ? 'w-0.5 bg-signal-ink' : 'rounded-[4px] border border-dashed border-signal-ink bg-signal/15',
      )}
    />
  );
}
