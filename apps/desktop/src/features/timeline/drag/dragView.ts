import type { RefObject } from 'react';
import type { TrackKind } from '../../../bindings/TrackKind';
import { useProject } from '../../../state/project';
import { useUi } from '../../../state/ui';
import { usToPx } from '../scale';
import type { DragView, RowHit } from './session';

/**
 * The timeline as a drag sees it: its scroller, the track rows a layer from `fromTrackId` can move to and the new-track
 * zone (only while every track shows), and the snap guide's parts and the tip, written through their style (never
 * state: a drag never re-renders).
 */
export function timelineDragView(
  scroller: HTMLElement,
  fromTrackId: string | null,
  guides: readonly RefObject<HTMLElement | null>[],
  tip: RefObject<HTMLElement | null>,
): DragView {
  // The scroller stays put during a drag: one layout read, at the press.
  const left = scroller.getBoundingClientRect().left;
  return {
    scroller,
    rowAt(clientY: number, kind: TrackKind): RowHit | null {
      if (fromTrackId === null || !useUi.getState().timelineExpanded) return null;
      let fromTop: number | null = null;
      let hit: { trackId: string; top: number } | null = null;
      for (const row of scroller.querySelectorAll<HTMLElement>(`[data-track-kind="${kind}"]`)) {
        const r = row.getBoundingClientRect();
        if (row.dataset.trackId === fromTrackId) fromTop = r.top;
        if (clientY >= r.top && clientY < r.bottom) hit = { trackId: row.dataset.trackId ?? '', top: r.top };
      }
      if (fromTop === null) return null;
      // The new-track zone first: a track of its own, appended to the project's tracks, which the view draws as the
      // top picture row or the bottom audio row, where the zone is.
      const zone = scroller.querySelector(`[data-new-track="${kind}"]`)?.getBoundingClientRect();
      if (zone && clientY >= zone.top && clientY < zone.bottom) {
        const tracks = useProject.getState().snapshot?.project.timeline.tracks.length ?? 0;
        return { newTrack: { index: tracks }, dy: zone.top - fromTop };
      }
      return hit ? { trackId: hit.trackId, dy: hit.top - fromTop } : null;
    },
    showGuide(us) {
      const x = us === null ? 0 : usToPx(us, useUi.getState().pxPerSecond);
      for (const { current: el } of guides) {
        if (!el) continue;
        el.style.display = us === null ? '' : 'block';
        if (us !== null) el.style.transform = `translateX(${x}px)`;
      }
    },
    showTip(text, clientX) {
      const el = tip.current;
      if (!el) return;
      el.style.display = text === null ? '' : 'block';
      if (text === null) return;
      if (el.textContent !== text) el.textContent = text;
      // The tip sits in the ruler row, which spans the scrolled content.
      if (clientX !== undefined)
        el.style.transform = `translateX(${clientX - left + scroller.scrollLeft}px) translateX(-50%)`;
    },
  };
}
