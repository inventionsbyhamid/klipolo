import { Trans, useLingui } from '@lingui/react/macro';
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from 'react';
import { snapToFrame } from '../../lib/time';
import { videoDurationUs } from '../../lib/timeline';
import { useStableCallback } from '../../lib/useStableCallback';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { keyBurst } from '../../state/keyBurst';
import { useRange } from '../../state/range';
import { useSelection } from '../../state/selection';
import { useUi } from '../../state/ui';
import { BlockMenu } from './blocks/BlockMenu';
import { blockPressHandlers } from './drag/blockPress';
import { cancelDrag, useDragged } from './drag/controller';
import { DropIndicator } from './DropIndicator';
import { registerDropTarget, timelineTargetAt } from './dropTarget';
import { useRovingFocus } from './keys/useRovingFocus';
import { RangeSelection, rangeDragJustEnded, startRangeDrag } from './lanes/RangeSelection';
import { BLOCK_HINT_ID, CLIP_INNER_PX, HEADER_PX, MAIN_LANE_MIN_PX, TAIL_US } from './layout';
import { LiveRegion } from './LiveRegion';
import { OverviewStrip } from './OverviewStrip';
import { Playhead } from './Playhead';
import { Ruler } from './ruler/Ruler';
import { pxToUs, usToPx } from './scale';
import { SnapGuide } from './SnapGuide';
import { TimelineLanes } from './TimelineLanes';
import { useFollowPlayhead } from './useFollowPlayhead';
import { useViewport } from './useViewport';
import { peekZoomScroll, registerZoomTarget, takeZoomScroll, zoomGestures } from './zoomControl';

/** The timeline panel's id. */
export const TIMELINE_PANEL_ID = 'timeline-panel';

/** The video's length now, for the zoom limits (read when a zoom runs, not on every render). */
function projectDurationUs(): number {
  const project = useProject.getState().snapshot?.project;
  return project ? videoDurationUs(project.timeline) : 0;
}

/**
 * A clip's inside (px) as the Video lane is laid out: it takes the timeline's spare height, so its filmstrip and
 * waveform are drawn for that. Read when the lane's size changes (jsdom has no layout: the default height).
 */
function useClipPx(scrollRef: RefObject<HTMLElement | null>): number {
  const [px, setPx] = useState(CLIP_INNER_PX);
  useLayoutEffect(() => {
    const lane = scrollRef.current?.querySelector<HTMLElement>('[data-lane="main"]');
    if (!lane) return;
    const read = () => {
      const height = lane.getBoundingClientRect().height;
      if (height >= MAIN_LANE_MIN_PX) setPx(Math.round(height) - 2);
    };
    read();
    const sizes = new ResizeObserver(read);
    sizes.observe(lane);
    return () => sizes.disconnect();
  }, [scrollRef]);
  return px;
}

/**
 * The project's lanes over time: the overview strip, the ruler, then Overlays, Video, Sounds and Music,
 * each with its icon in the gutter. Press a block to select it (the playhead jumps to its start unless it's already
 * inside, so S splits where the person was) and drag it to move it (`drag/controller`), or drag one of its edges to
 * trim it (`drag/trimFrame`); click empty space or the ruler to seek, drag across them to select a time range
 * (`lanes/RangeSelection`), which Delete takes out of the video.
 *
 * Only the blocks in the viewport's window render, and they are `memo`'d on primitives with a stable `onPick`, so an
 * edit re-renders only the blocks it changed. The keyboard reaches every block (`keys/useRovingFocus`), and every
 * block has a menu (`blocks/BlockMenu`); edits run as commands (`useTimelineCommands`).
 *
 * Pinch (or Command + wheel) zooms around the pointer; the player bar and shortcuts zoom around the playhead
 * (`zoomControl`). Media dragged from the library or Finder shows where it would land (`dropTarget`, `DropIndicator`).
 */
export function TimelineView() {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot!.project);
  const expanded = useUi((s) => s.timelineExpanded);
  const pps = useUi((s) => s.pxPerSecond);
  const seek = usePlayback((s) => s.seek);
  const selectedId = useSelection((s) => s.selectedId);
  const select = useSelection((s) => s.select);
  const dragged = useDragged((s) => s.ids);
  const scrollRef = useRef<HTMLDivElement>(null);
  const guideRef = useRef<HTMLDivElement>(null);
  const tipRef = useRef<HTMLDivElement>(null);
  const headRef = useRef<HTMLDivElement>(null);
  // The blocks' pick and press handlers: one set for the timeline's lifetime, so a `memo` block never re-renders.
  const [{ onPick, onPress, onTrimPress }] = useState(() => blockPressHandlers(scrollRef, guideRef, tipRef));
  // A zoom's render puts its anchor back under its x as soon as the new width is laid out. Declared before
  // `useViewport`, so its layout effect runs first and the viewport's window is the one on screen.
  useLayoutEffect(() => {
    const scroll = takeZoomScroll();
    if (scroll !== null && scrollRef.current) scrollRef.current.scrollLeft = scroll;
  }, [pps]);
  // A zoom's render already draws the window at the scroll it is about to take (no extra blocks for one render).
  const viewport = useViewport(scrollRef, pps, HEADER_PX, peekZoomScroll());
  useFollowPlayhead(scrollRef, pps, HEADER_PX);
  const clipPx = useClipPx(scrollRef);
  const roving = useRovingFocus(scrollRef, project, expanded, selectedId, pps, viewport.scrollPx);
  const { tabStopId } = roving;
  // Selected, focused (the tab stop) and dragged blocks stay mounted and never band: a drag keeps its element through
  // autoscroll, focus never drops to the page, and the keyboard reaches blocks off-window or in a band.
  const keep = useMemo(
    () => new Set([selectedId, tabStopId, ...dragged].filter((id) => id !== null)),
    [selectedId, tabStopId, dragged],
  );

  useLayoutEffect(() => {
    const scroller = scrollRef.current;
    if (!scroller) return;
    registerZoomTarget({
      scroller,
      headerPx: HEADER_PX,
      durationUs: projectDurationUs,
      fps: () => useProject.getState().snapshot?.project.settings.fps ?? 30,
    });
    const offGestures = zoomGestures(scroller);
    // Media dragged from the library or Finder lands where the pointer is over the lanes (below the ruler).
    registerDropTarget(
      (x, y, kind) =>
        timelineTargetAt(scroller, headRef.current?.getBoundingClientRect().bottom ?? 0, x, y, kind),
      scroller,
    );
    return () => {
      offGestures();
      registerZoomTarget(null);
      registerDropTarget(null);
    };
  }, []);
  // One selection at a time: a block picked clears the time range (making a range clears the block's).
  useEffect(
    () =>
      useSelection.subscribe((s) => {
        if (s.selectedId !== null && useRange.getState().range) useRange.getState().clear();
      }),
    [],
  );
  // Unmounted mid-drag: the gesture under way ends (nothing stays shifted or dry-run), and a nudge settling commits.
  useEffect(
    () => () => {
      cancelDrag();
      keyBurst.flush();
    },
    [],
  );

  // The video's length: to the main track's or its last layer's end, whichever is later, so the ruler and the
  // playhead reach a layer past the main track.
  const durationUs = videoDurationUs(project.timeline);
  const laneWidth = Math.max(viewport.widthPx - HEADER_PX, usToPx(durationUs + TAIL_US, pps));
  const fps = project.settings.fps;
  const empty = project.timeline.main.length === 0 && project.timeline.tracks.every((tr) => tr.items.length === 0);

  // Seeks stay on the frame grid and within the video. Stable, so the `memo` ruler skips edits.
  const seekTo = useStableCallback((px: number) =>
    seek(Math.min(snapToFrame(pxToUs(Math.max(0, px), pps), fps), durationUs)),
  );
  // A click on empty lane: nothing selected, the playhead there (unless the press made a time range).
  const seekPx = (px: number) => {
    if (rangeDragJustEnded()) return;
    select(null);
    useRange.getState().clear();
    seekTo(px);
  };
  const view = {
    pps,
    fromUs: viewport.fromUs,
    toUs: viewport.toUs,
    selectedId,
    tabStopId,
    keep,
    onPick,
    onPress,
    onTrimPress,
  };
  return (
    <section
      id={TIMELINE_PANEL_ID}
      aria-label={t({ id: 'timeline.label', message: 'Timeline' })}
      data-timeline
      className="panel timeline"
    >
      <div className="tl-panel">
        <OverviewStrip main={project.timeline.main} durationUs={durationUs} pps={pps} scrollRef={scrollRef} />
        <BlockMenu>
          <div
            ref={scrollRef}
            // The keys between blocks (event delegation), and where focus is.
            onKeyDown={roving.onKeyDown}
            onKeyUp={roving.onKeyUp}
            onFocus={roving.onFocus}
            onBlur={roving.onBlur}
            // A press on empty lane space starts a time range (blocks and their handles take their own presses).
            onPointerDown={(e) => {
              const lane = (e.target as Element).closest<HTMLElement>('.tl-lane');
              if (lane && e.target === lane) startRangeDrag(e, lane);
            }}
            className="tl-scroll overflow-auto"
          >
            {/* At least the view's width: while a window drag holds the lanes' window (`useViewport`), the lanes and
                the ruler stretch to the edge in CSS. */}
            <div className="tl-content" style={{ width: HEADER_PX + laneWidth }}>
              <div ref={headRef} className="tl-head">
                <div className="tl-gutter" style={{ width: HEADER_PX }} />
                <Ruler width={laneWidth} pps={pps} fps={fps} viewport={viewport} onScrub={seekTo} />
                {/* Where a dragged layer lands, or a dragged clip's position; drawn by the drag (`showTip`). */}
                <div ref={tipRef} aria-hidden className="tl-tip" />
              </div>
              <TimelineLanes
                project={project}
                viewport={viewport}
                view={view}
                width={laneWidth}
                clipPx={clipPx}
                onSeek={seekPx}
              />
              {/* Over the blocks, under the gutter: the time range and the playhead. */}
              <div className="tl-marks" style={{ left: HEADER_PX, width: laneWidth }}>
                <RangeSelection pps={pps} />
                <Playhead pps={pps} />
                <SnapGuide ref={guideRef} />
                <DropIndicator pps={pps} />
              </div>
            </div>
          </div>
        </BlockMenu>
        {empty && (
          <p className="tl-empty">
            <Trans id="timeline.empty" comment="Shown on the empty timeline">
              Clips line up here in the order you add them.
            </Trans>
          </p>
        )}
      </div>
      <LiveRegion />
      {/* What each block's description says (`aria-describedby`): the keys that move between blocks. */}
      <p id={BLOCK_HINT_ID} hidden>
        <Trans id="timeline.blocks.keysHint" comment="Read by screen readers after a clip's or layer's name">
          Arrow keys move between clips and layers. Shift+F10 shows what you can do.
        </Trans>
      </p>
    </section>
  );
}
