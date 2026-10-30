import { create } from 'zustand';
import { clockAnchor, clockUs } from '../../state/playClock';
import { usePlayback } from '../../state/playback';
import { useUi } from '../../state/ui';
import { fitPps, scrollForAnchor, zoomLimits } from './model/zoom';
import { pxToUs, usToPx } from './scale';

/** What zooms: the timeline registers itself, and commands and shortcuts zoom it without holding a ref. */
interface ZoomTarget {
  scroller: HTMLElement;
  headerPx: number;
  durationUs: () => number;
  fps: () => number;
}

let target: ZoomTarget | null = null;
/** The scroll that puts a zoom's anchor back under its x, once the timeline has laid out the new width. */
let pendingScroll: number | null = null;

/** Bumped when a timeline registers or goes, so the player bar's zoom slider reads its range again. */
export const useZoomTarget = create(() => ({ version: 0 }));

export function registerZoomTarget(next: ZoomTarget | null) {
  target = next;
  pendingScroll = null;
  useZoomTarget.setState((s) => ({ version: s.version + 1 }));
}

/**
 * The scroll the last zoom asked for (once). The timeline applies it in a layout effect on the zoom's render, before
 * its viewport reads the scroll: the viewport's window is the one on screen, and the next pinch event anchors right.
 */
export function takeZoomScroll(): number | null {
  const scroll = pendingScroll;
  pendingScroll = null;
  return scroll;
}

/**
 * The scroll a zoom about to render will take, without taking it: that render draws the window there (`useViewport`).
 */
export function peekZoomScroll(): number | null {
  return pendingScroll;
}

/**
 * The time under `clientX` and its x in the lanes (never left of them), for zooming around the pointer. None without
 * a pointer position (a gesture that carries none), so the zoom keeps the playhead or the centre instead.
 */
export function pointerAnchor(clientX: number | undefined): { us: number; x: number } | undefined {
  if (!target || clientX === undefined || !Number.isFinite(clientX)) return undefined;
  const { scroller, headerPx } = target;
  const x = Math.max(0, clientX - scroller.getBoundingClientRect().left - headerPx);
  return { us: pxToUs(x + scroller.scrollLeft, useUi.getState().pxPerSecond), x };
}

/** Where the playhead is drawn: the running clock while playing, else the stored time. */
function playheadNowUs(): number {
  const { playing, playheadUs } = usePlayback.getState();
  return playing && clockAnchor().running ? clockUs() : playheadUs;
}

/**
 * Zoom by `factor` around `anchor` (time and its x in the lanes). With no anchor: around the playhead when it is
 * on screen, else the view's centre. Zooming in at or past the closest zoom (a zoom kept from a 60 fps project in a
 * 30 fps one), or out at or past the farthest, does nothing rather than going the other way.
 */
export function zoomTimeline(factor: number, anchor?: { us: number; x: number }) {
  const ui = useUi.getState();
  if (!target) return ui.setPxPerSecond(ui.pxPerSecond * factor);
  const { scroller, headerPx } = target;
  const view = scroller.clientWidth - headerPx;
  const [min, max] = zoomLimits(target.durationUs(), view, target.fps());
  if ((factor > 1 && ui.pxPerSecond >= max) || (factor < 1 && ui.pxPerSecond <= min)) return;
  const pps = Math.min(max, Math.max(min, ui.pxPerSecond * factor));
  const playheadUs = playheadNowUs();
  const playheadX = usToPx(playheadUs, ui.pxPerSecond) - scroller.scrollLeft;
  const at =
    anchor ??
    (playheadX >= 0 && playheadX <= view
      ? { us: playheadUs, x: playheadX }
      : { us: pxToUs(scroller.scrollLeft + view / 2, ui.pxPerSecond), x: view / 2 });
  ui.setPxPerSecond(pps);
  // The store's value (it has absolute bounds too). The render comes after this returns, so it finds the scroll; at a
  // limit already, nothing moved and no render would take it.
  const now = useUi.getState().pxPerSecond;
  pendingScroll = now === ui.pxPerSecond ? null : scrollForAnchor(at.us, at.x, now);
}

/** The zoom's range now (px per second, farthest and closest), or null while no timeline is on screen. */
export function zoomRange(): [number, number] | null {
  if (!target) return null;
  return zoomLimits(target.durationUs(), target.scroller.clientWidth - target.headerPx, target.fps());
}

/** How much of the video the lanes show at the zoom now (µs), or null while no timeline is on screen. */
export function zoomViewUs(): number | null {
  if (!target) return null;
  return pxToUs(Math.max(0, target.scroller.clientWidth - target.headerPx), useUi.getState().pxPerSecond);
}

/** Zoom to `pps` (the player bar's slider), around the playhead when it is on screen, else the view's centre. */
export function zoomTo(pps: number) {
  const now = useUi.getState().pxPerSecond;
  if (pps > 0 && now > 0 && pps !== now) zoomTimeline(pps / now);
}

/** The whole project in view, from the start (no closer than 40 px per frame for a very short one). */
export function fitTimeline() {
  if (!target) return;
  const view = target.scroller.clientWidth - target.headerPx;
  const durationUs = target.durationUs();
  const [, max] = zoomLimits(durationUs, view, target.fps());
  const before = useUi.getState().pxPerSecond;
  useUi.getState().setPxPerSecond(Math.min(max, fitPps(durationUs, view)));
  // Valid at the old width too, so it's set now; and pending, so the fit's render draws its window at 0.
  pendingScroll = useUi.getState().pxPerSecond === before ? null : 0;
  target.scroller.scrollLeft = 0;
}

/** How long after a WebKit pinch's last event a ctrl+wheel still counts as part of it (ms). */
const PINCH_TRAILING_MS = 150;

/**
 * Pinch and ⌘/ctrl + wheel on `scroller` zoom around the pointer; any other wheel scrolls natively. A trackpad pinch
 * arrives as ctrl+wheel in Chromium and WebView2, and as GestureEvents (with `scale` since the gesture began) in
 * WKWebView. At most one zoom per frame: the events since the last frame make one step to where they head, around the
 * latest pointer (nothing moves between them, so the anchor is exact). Returns the cleanup.
 */
export function zoomGestures(scroller: HTMLElement): () => void {
  let towardPps: number | null = null;
  let towardX: number | undefined;
  let frame = 0;
  const queue = (pps: number, clientX: number | undefined) => {
    towardPps = pps;
    towardX = clientX;
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      const to = towardPps;
      towardPps = null;
      if (to !== null) zoomTimeline(to / useUi.getState().pxPerSecond, pointerAnchor(towardX));
    });
  };

  // If WebKit sent ctrl+wheel for a pinch as well as its GestureEvents, the pinch would zoom twice: until a moment
  // after the gesture's last event, wheels only stop the page zooming. Its last event, not `gestureend`: a gesture
  // that never ends must not turn ⌘ + wheel off for good. Timed on one clock (`performance.now()`), not two event
  // types' timestamps.
  let gestureUntil = -Infinity;
  const inGesture = () => {
    gestureUntil = performance.now() + PINCH_TRAILING_MS;
  };
  const onWheel = (e: WheelEvent) => {
    if (!e.ctrlKey && !e.metaKey) return;
    e.preventDefault();
    if (performance.now() < gestureUntil) return;
    queue((towardPps ?? useUi.getState().pxPerSecond) * Math.exp(-e.deltaY * 0.01), e.clientX);
  };
  // WKWebView (macOS): GestureEvents aren't in TypeScript's DOM types.
  type Pinch = Event & { scale?: number; clientX?: number };
  let pinchFrom = 0;
  const onGestureStart = (e: Event) => {
    e.preventDefault();
    inGesture();
    pinchFrom = useUi.getState().pxPerSecond;
  };
  const onGestureChange = (e: Event) => {
    e.preventDefault();
    inGesture();
    const { scale, clientX } = e as Pinch;
    if (scale !== undefined && Number.isFinite(scale) && scale > 0) queue(pinchFrom * scale, clientX);
  };
  const onGestureEnd = (e: Event) => {
    e.preventDefault();
    inGesture();
  };
  scroller.addEventListener('wheel', onWheel, { passive: false });
  scroller.addEventListener('gesturestart', onGestureStart);
  scroller.addEventListener('gesturechange', onGestureChange);
  scroller.addEventListener('gestureend', onGestureEnd);
  return () => {
    cancelAnimationFrame(frame);
    scroller.removeEventListener('wheel', onWheel);
    scroller.removeEventListener('gesturestart', onGestureStart);
    scroller.removeEventListener('gesturechange', onGestureChange);
    scroller.removeEventListener('gestureend', onGestureEnd);
  };
}
