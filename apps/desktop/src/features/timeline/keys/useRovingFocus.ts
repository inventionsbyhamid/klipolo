import {
  useEffect,
  useLayoutEffect,
  useRef,
  type FocusEvent,
  type KeyboardEvent,
  type RefObject,
} from 'react';
import type { Project } from '../../../bindings/Project';
import { findItem, videoDurationUs } from '../../../lib/timeline';
import { usePlayback } from '../../../state/playback';
import { useProject } from '../../../state/project';
import { groupOf, useSelection } from '../../../state/selection';
import { useTimelineFocus } from '../../../state/timelineFocus';
import { keyboardMenuOpen, openBlockMenu } from '../blocks/BlockMenu';
import { pickBlock } from '../drag/blockPress';
import { blockElement } from '../drag/registry';
import { clipIndexAt, mainIndex } from '../model/index';
import { pxToUs } from '../scale';
import {
  cutClipId,
  focusIdOf,
  focusSelection,
  LANE_KEYS,
  laneOrder,
  nearestIn,
  placeOf,
  stepFromHidden,
  type LaneKey,
} from './laneOrder';
import { blockOf, inProject, neighbour, pageFocused } from './focusHelpers';
import { selectEveryBlock } from './groupActions';
import { isMarker, markerFocusedIn, markerFor } from './markers';
import { reveal, revealRow } from './reveal';
import { focusStep, moveFocusTo } from './timelineActions';
import { enterCut } from './transitionActions';

const MOVES = new Set<string>(Object.values(LANE_KEYS));
/** A Mac, where ⌘ selects everything; Windows and Linux use Ctrl (as the editor's shortcuts read it). */
const MAC = typeof navigator === 'undefined' || !/^(Win|Linux)/.test(navigator.platform);

/**
 * Roving focus over the timeline's blocks: one block is in the Tab order (`tabStopId`: the one last focused or
 * selected, else the first clip in view); ← / → move along its lane, ↑ / ↓ between lanes, Home / End to a lane's ends
 * (selection follows; the Video lane stops at each cut between clips), Enter picks (selects and seeks; on a cut it
 * selects its transition, adds one, or says why it can't), Space plays, Shift+F10 or the Menu key opens its menu.
 *
 * The tab stop is kept mounted and out of bands (`keep`), so a focused block survives scrolling and zooming, and off-
 * window or banded blocks are reachable. Focus never drops to the page: a focused block that goes (deleted, cut,
 * undone, the other track view) hands focus to its lane neighbour, or to the tab stop.
 */
export function useRovingFocus(
  scrollRef: RefObject<HTMLElement | null>,
  project: Project,
  selectedId: string | null,
  pps: number,
  scrollPx: number,
) {
  const activeId = useTimelineFocus((s) => s.activeId);
  // Every keyboard move renders (even one to the block already in the Tab order), so its focus moves.
  useTimelineFocus((s) => s.moves);
  const lanes = laneOrder(project);
  const before = useRef(lanes);
  /** A block had focus and it hasn't left the timeline (a block taken out of the page doesn't count as leaving). */
  const hadFocus = useRef(false);
  /** The packed layer whose "+N" marker has focus (it stands for that layer, which stays selected). */
  const hidden = useRef<{ id: string; startUs: number } | null>(null);

  let tabStopId: string | null = activeId !== null && placeOf(lanes, activeId) ? activeId : null;
  if (tabStopId === null) {
    const { main } = project.timeline;
    const inView = clipIndexAt(mainIndex(main), pxToUs(scrollPx, pps));
    // A selected transition is reached at its cut.
    const selectedAt = focusIdOf(selectedId);
    tabStopId =
      selectedAt !== null && placeOf(lanes, selectedAt)
        ? selectedAt
        : main.length > 0
          ? main[inView >= 0 ? inView : main.length - 1].id
          : (lanes.find((l) => l.ids.length > 0)?.ids[0] ?? null);
  }

  // A selection made elsewhere (the preview, the palette, a paste) becomes the block Tab enters at.
  useEffect(
    () =>
      useSelection.subscribe((s, prev) => {
        const id = focusIdOf(s.selectedId);
        const now = useProject.getState().snapshot?.project;
        if (s.selectedId === prev.selectedId || id === null || !now) return;
        if (placeOf(laneOrder(now), id))
          useTimelineFocus.getState().focus(id, false);
      }),
    [],
  );

  // After every render: a block that is gone hands the tab stop to its neighbour; the keyboard's move takes focus to
  // its block (mounted by now); and focus that dropped to the page comes back to the tab stop.
  useLayoutEffect(() => {
    const scroller = scrollRef.current;
    const focus = useTimelineFocus.getState();
    const last = before.current;
    before.current = lanes;
    // Focus on the "+N" marker of a packed layer goes back to the layer once it's drawn again (every track shown, or
    // moved out), and on to its neighbour, the selection too, once it's deleted (⌫ there). If the marker went (its run
    // scrolled out of the window), focus goes to the block in view and the selection with it: the two never point at
    // different things, so ⌫ deletes only what has focus. Focus moved elsewhere, or the timeline lost it (a click on
    // the preview, on a button), ends it: then nothing pulls focus back into the timeline.
    // A keyboard move to another block (a copy ⌘D or ⌘V made from the marker) takes over from the held layer.
    if (focus.pending && focus.activeId !== null && focus.activeId !== hidden.current?.id)
      hidden.current = null;
    const held = hidden.current;
    if (held && scroller) {
      const onMarker = markerFocusedIn(scroller);
      if (!onMarker && (!pageFocused() || !hadFocus.current)) hidden.current = null;
      else if (placeOf(lanes, held.id)) {
        hidden.current = null;
        focus.focus(held.id, true);
        return;
      } else if (!inProject(project, held.id)) {
        hidden.current = null;
        const to = nearestIn(lanes, 'layers', held.startUs) ?? nearestIn(lanes, 'main', held.startUs);
        if (to !== null) moveFocusTo(project, to);
        return;
      } else if (!onMarker) {
        const item = findItem(project, held.id)?.item;
        const marker = item ? markerFor(scroller, item) : null;
        if (marker) {
          // It was drawn again (re-mounted): focus goes back to it.
          marker.focus({ preventScroll: true });
          return;
        }
        hidden.current = null;
        if (tabStopId !== null) moveFocusTo(project, tabStopId);
        return;
      }
    }
    if (focus.activeId !== null && !placeOf(lanes, focus.activeId)) {
      const gone = focus.activeId;
      // Packed into the Overlays lane's "+N" (it's still in the project): focus goes to the marker standing for it,
      // which names it, and the selection to the layer, so the two never point at different things. Enter there picks
      // it; ⌫ deletes it. From a marker too: a copy made there, packed in turn.
      const item = findItem(project, gone)?.item;
      const onMarker = !!scroller && markerFocusedIn(scroller);
      const marker =
        item && scroller && hadFocus.current && (pageFocused() || onMarker)
          ? markerFor(scroller, item)
          : null;
      if (item && marker) {
        hidden.current = { id: gone, startUs: item.startUs };
        focus.focus(null, false);
        useSelection.getState().select(gone);
        marker.focus({ preventScroll: true });
        return;
      }
      // The neighbour may not be mounted yet: the next render (it's kept now) focuses it.
      const to = neighbour(last, lanes, gone);
      focus.focus(to, false);
      // ⌫ or ⌘X on the focused block, or a block packed into "+N" with no marker in view: the selection follows
      // focus, so a second ⌫ deletes the block that has it, never one out of sight.
      const selection = useSelection.getState();
      if (to !== null && hadFocus.current && focusIdOf(selection.selectedId) === gone)
        selection.select(focusSelection(project, to));
      return;
    }
    if (!scroller) return;
    if (focus.pending && focus.activeId !== null) {
      // Settled even if the block can't be found, so a stale move never takes focus later.
      const el = blockElement(focus.activeId);
      focus.settle();
      el?.focus({ preventScroll: true });
      if (el) revealRow(scroller, el);
    } else if (hadFocus.current && pageFocused() && tabStopId !== null) {
      blockElement(tabStopId)?.focus({ preventScroll: true });
    }
  });

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    const el = blockOf(e.target);
    const id = el?.dataset.blockId;
    const now = useProject.getState().snapshot?.project;
    // ⌘A (Ctrl A off a Mac) while the timeline has focus: every clip and layer.
    const all = (MAC ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey) && !e.altKey && !e.shiftKey;
    if (now && all && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      return selectEveryBlock();
    }
    if (!now || e.metaKey || e.ctrlKey || e.altKey) return;
    const held = hidden.current;
    if (held && isMarker(e.target) && MOVES.has(e.key) && !e.shiftKey) {
      // On the "+N" marker of a packed layer, the arrows go from where that layer is.
      e.preventDefault();
      const lanes = laneOrder(now);
      const next = stepFromHidden(lanes, 'layers', held.id, held.startUs, e.key as LaneKey);
      if (next === null) return;
      if (scrollRef.current) reveal(scrollRef.current, lanes, next, pps);
      return moveFocusTo(now, next);
    }
    if (!el || !id) return;
    // With several blocks selected, ← and → nudge them (the window's shortcut), and focus stays where it is.
    const sideways = e.key === 'ArrowLeft' || e.key === 'ArrowRight';
    if (sideways && groupOf(useSelection.getState())) return;
    if (MOVES.has(e.key) && !e.shiftKey) {
      // The window's shortcuts (a frame, an edit point, the start or end) don't also run.
      e.preventDefault();
      const next = focusStep(now, id, e.key as LaneKey);
      if (next === null || next === id) return;
      if (scrollRef.current) reveal(scrollRef.current, laneOrder(now), next, pps);
      moveFocusTo(now, next);
    } else if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      // On a cut: its transition is selected, or one is added, or it says why not.
      const cutOf = cutClipId(id);
      if (cutOf !== null) enterCut(now, cutOf);
      else pickBlock(id);
    } else if (e.key === ' ') {
      // Space plays from a block too (a button would take it as a click).
      e.preventDefault();
      if (videoDurationUs(now.timeline) > 0) usePlayback.getState().toggle();
    } else if ((e.key === 'F10' && e.shiftKey) || e.key === 'ContextMenu') {
      e.preventDefault();
      openBlockMenu(el, scrollRef.current);
    }
  };

  /** Space's release would click the block (WebKit clicks a button on keyup). */
  const onKeyUp = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key === ' ' && blockOf(e.target)) e.preventDefault();
  };

  const onFocus = (e: FocusEvent<HTMLElement>) => {
    hadFocus.current = true;
    const id = blockOf(e.target)?.dataset.blockId;
    if (!id) return;
    // Off the "+N" marker of a packed, selected layer (Tab, a click): the selection comes with focus, so ⌫ deletes
    // the block that has it, never the one out of sight. So too when focus comes in (Tab from the preview) while the
    // selection is something the lanes don't draw (a layer packed into "+N", a camera): the focused block is selected.
    const now = useProject.getState().snapshot?.project;
    const selected = useSelection.getState().selectedId;
    const undrawn =
      selected !== null && now !== undefined && !placeOf(laneOrder(now), focusIdOf(selected));
    if ((hidden.current || undrawn) && now) {
      hidden.current = null;
      useSelection.getState().select(focusSelection(now, id));
    }
    useTimelineFocus.getState().focus(id, false);
  };

  const onBlur = (e: FocusEvent<HTMLElement>) => {
    const scroller = e.currentTarget;
    // Into the block menu the keyboard opened: still the timeline's (a Delete there moves focus on, as ⌫ does).
    if ((e.relatedTarget instanceof Node && scroller.contains(e.relatedTarget)) || keyboardMenuOpen()) return;
    // Focus left the timeline only if it isn't back in it once this has settled. An engine that blurs a block React
    // moves or takes out (Chromium, WebView2) leaves focus on the moved block, or the effect above brings it back.
    queueMicrotask(() => {
      if (!scroller.contains(document.activeElement)) hadFocus.current = false;
    });
  };

  return { tabStopId, onKeyDown, onKeyUp, onFocus, onBlur };
}
