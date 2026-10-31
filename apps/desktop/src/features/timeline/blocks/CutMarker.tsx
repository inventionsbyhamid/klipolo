import { useLingui } from '@lingui/react/macro';
import { memo, useCallback, type MouseEvent, type PointerEvent } from 'react';
import type { Edge } from '../../../bindings/Edge';
import { cn } from '../../../design/cn';
import { useIntlLocale } from '../../../i18n/react';
import { Icon } from '../../../layout/Icon';
import { formatSeconds } from '../../../lib/format';
import { countRender } from '../../../lib/renderCount';
import { splitDuration } from '../../../lib/transitions';
import { registerBlock } from '../drag/registry';
import { DRAG_SLOP_PX } from '../drag/session';
import {
  onBlockPointerCancel,
  onBlockPointerMove,
  onBlockPointerUp,
  type PressTrim,
} from '../drag/useBlockDrag';
import { cutId } from '../keys/laneOrder';
import { addTip, transitionLabels, type FadesReplaced } from '../keys/transitionActions';
import { usToPx } from '../scale';

/** A transition's chip is never narrower than this (px), so a short one zoomed out can still be clicked. */
export const CHIP_NARROWEST_PX = 14;
/** A chip this wide (px) has room for its icon. */
const CHIP_ICON_PX = 18;
/** A bare cut's spot (px): its Add transition button or note, as wide as a fingertip's aim needs. */
const SPOT_PX = 20;
/**
 * How far (px) either side of its cut a press on a marker is the trim's when it drags: the part of a clip's 12 px trim
 * pad inside the clip (`.tl-trim`, 4 px of it hangs past the edge).
 */
const TRIM_REACH_PX = 8;

interface CutMarkerProps {
  /** The clip before the cut (the transition is its `transitionOut`). */
  clipId: string;
  cutUs: number;
  pps: number;
  /** The clips' names either side of the cut. */
  before: string;
  after: string;
  /** Its transition's name ("Dissolve", "Missing transition"), or null when it has none. */
  transitionName: string | null;
  /** What it is, named whole: "Dissolve transition", or "Missing transition" for one this build lacks. */
  transitionKind: string;
  transitionUs: number;
  /** A transition this build lacks: its id, as a tooltip. */
  transitionTitle?: string;
  /** The clip after the cut: a drag from a press right of the cut trims its start. */
  nextClipId: string;
  /** Whether a transition fits here (spare footage either side). */
  canAdd: boolean;
  /** The fades at this cut a new transition would replace (the model drops them). */
  replaces: FadesReplaced;
  /** Its transition is selected. */
  selected: boolean;
  /** The one block in the Tab order (roving focus). */
  tabbable: boolean;
  onAdd: (clipId: string) => void;
  onSelect: (clipId: string) => void;
  /** A bare cut without room: a click says why. */
  onNoRoom: (clipId: string) => void;
  /** The lane's press on a clip's edge (`blockPress`'s `onTrimPress`), which a drag from near the cut is passed to. */
  onTrimPress: PressTrim;
}

/** A press near a marker's cut, held until it moves (a trim) or is released (the cut's own click). One at a time. */
let held: { down: PointerEvent<HTMLElement>; clipId: string; edge: Edge } | null = null;
/** The last press was passed to a trim: the click its release makes is not the marker's. */
let handedOff = false;

/** After a press passed to a trim, the click its release makes (on the lane, which would seek) does nothing. */
function swallowNextClick() {
  const swallow = (e: Event) => {
    e.stopPropagation();
    e.preventDefault();
    stop();
  };
  const later = () => setTimeout(stop, 0);
  function stop() {
    // The gesture is over: a later click on a cut (one with no press too, as VoiceOver's) is the cut's own.
    handedOff = false;
    window.removeEventListener('click', swallow, true);
    window.removeEventListener('pointerup', later, true);
    window.removeEventListener('pointercancel', later, true);
  }
  window.addEventListener('click', swallow, true);
  window.addEventListener('pointerup', later, true);
  window.addEventListener('pointercancel', later, true);
}

/**
 * A cut on the Video lane, along the lane's bottom edge, clear of the clips' names at the top and of the trim
 * grips in the middle of their edges.
 * - Its transition: a chip centred on the cut and as wide as it lasts.
 * - Else an "Add transition" button, shown only while its spot is pointed at or has focus.
 * - Else, where neither clip has footage to spare, a note saying so, shown the same way.
 *
 * It covers the bottom of the clips' edges, so a press there within `TRIM_REACH_PX` of the cut is held: its first move
 * of `DRAG_SLOP_PX` or more passes it to the trim of the clip on that side (the end of the one before, the start of
 * the one after), with nothing focused or selected before that; only a still click acts on the cut. A press farther
 * along a wide chip takes focus at once and selects on its click.
 *
 * It is a stop on the lane's roving focus (`cut:<clip id>`): ← / → reach it between the clips, Enter selects, adds
 * or says why. Memoized on primitives, like the blocks; its tooltip is the lane's one `CutTip` (`data-tip`).
 */
export const CutMarker = memo(function CutMarker({
  clipId,
  cutUs,
  pps,
  before,
  after,
  transitionName,
  transitionKind,
  transitionUs,
  transitionTitle,
  canAdd,
  replaces,
  selected,
  tabbable,
  onAdd,
  onSelect,
  onNoRoom,
  nextClipId,
  onTrimPress,
}: CutMarkerProps) {
  const id = cutId(clipId);
  countRender(id);
  const { t } = useLingui();
  const locale = useIntlLocale();
  const ref = useCallback((el: HTMLElement | null) => registerBlock(id, el), [id]);
  const at = usToPx(cutUs, pps);
  /** Its press, moves and release, for a marker whose cut is `cutX` px into it. */
  const pointer = (cutX: number) => ({
    onPointerDown: (e: PointerEvent<HTMLElement>) => {
      if (e.button !== 0) return;
      // No focus or text selection from the press itself, and never the lane's (a seek, a time range).
      e.preventDefault();
      e.stopPropagation();
      handedOff = false;
      const fromCut = e.clientX - e.currentTarget.getBoundingClientRect().left - cutX;
      if (Math.abs(fromCut) > TRIM_REACH_PX) {
        held = null;
        // Takes focus, as a block's press does (WebKit doesn't focus buttons on click).
        e.currentTarget.focus({ preventScroll: true });
        return;
      }
      e.currentTarget.setPointerCapture?.(e.pointerId);
      held = fromCut < 0 ? { down: e, clipId, edge: 'end' } : { down: e, clipId: nextClipId, edge: 'start' };
    },
    onPointerMove: (e: PointerEvent<HTMLElement>) => {
      if (handedOff) return onBlockPointerMove(e);
      // No button down: a hold whose release the marker never heard (it went mid-hold) is over; a hover never trims.
      if ((e.buttons & 1) === 0) {
        held = null;
        return;
      }
      const press = held;
      if (!press || e.pointerId !== press.down.pointerId) return;
      if (Math.hypot(e.clientX - press.down.clientX, e.clientY - press.down.clientY) < DRAG_SLOP_PX) return;
      held = null;
      handedOff = true;
      swallowNextClick();
      // The trim takes the pointer, focus and selection from here, as a press on that clip's edge would.
      onTrimPress(press.down, press.clipId, press.edge);
      onBlockPointerMove(e);
    },
    onPointerUp: (e: PointerEvent<HTMLElement>) => {
      held = null;
      if (handedOff) onBlockPointerUp(e);
    },
    onPointerCancel: () => {
      held = null;
      if (handedOff) onBlockPointerCancel();
    },
  });
  const common = { ref, 'data-block-id': id, tabIndex: tabbable ? 0 : -1 };
  /** A click (a keyboard's Enter is the lane's: `useRovingFocus`) takes focus and acts, and never seeks the lane. */
  const onClick = (act: () => void) => (e: MouseEvent<HTMLElement>) => {
    e.stopPropagation();
    if (handedOff) {
      handedOff = false;
      return;
    }
    e.currentTarget.focus({ preventScroll: true });
    act();
  };

  if (transitionName !== null) {
    const exact = usToPx(transitionUs, pps);
    const width = Math.max(CHIP_NARROWEST_PX, exact);
    // Its window starts half its length before the cut; widened to the minimum, it stays centred on it.
    const left = usToPx(cutUs - splitDuration(transitionUs)[0], pps) - (width - exact) / 2;
    const length = formatSeconds(locale, transitionUs / 1e6);
    return (
      <button
        type="button"
        {...common}
        {...pointer(at - left)}
        data-tip={t({ id: 'transition.chip.tip', message: `${transitionName}, ${length}` })}
        aria-label={t({
          id: 'transition.chip.name',
          message: `${transitionKind}, ${length}, between ${before} and ${after}`,
          comment:
            'A transition on the timeline; {transitionKind} is e.g. “Dissolve transition”, {length} e.g. 0.5 s',
        })}
        aria-pressed={selected}
        title={transitionTitle}
        onClick={onClick(() => onSelect(clipId))}
        style={{ left, width }}
        className={cn('tl-transition', selected && 'is-selected')}
      >
        {width >= CHIP_ICON_PX && <Icon name="blend" />}
      </button>
    );
  }
  if (canAdd) {
    return (
      <button
        type="button"
        {...common}
        {...pointer(SPOT_PX / 2)}
        data-tip={addTip(replaces)}
        aria-label={t({
          id: 'transition.addBetween',
          message: `Add transition between ${before} and ${after}`,
          comment: '{before} and {after} are the names of the clips either side of a cut',
        })}
        onClick={onClick(() => onAdd(clipId))}
        style={{ left: at - SPOT_PX / 2, width: SPOT_PX }}
        // Shown only while its own spot is pointed at or has focus (never for a hover elsewhere on the lane).
        className="tl-cut-spot"
      >
        <span className="tl-cut-plus">
          <Icon name="plus" />
        </span>
      </button>
    );
  }
  return (
    <button
      type="button"
      {...common}
      {...pointer(SPOT_PX / 2)}
      data-tip={transitionLabels.noRoom()}
      aria-disabled
      aria-label={t({
        id: 'transition.noRoomAt',
        message: `Cut between ${before} and ${after}. Not enough extra footage here for a transition`,
        comment: 'A cut where no transition fits: the clips have no footage past their ends to blend',
      })}
      onClick={onClick(() => onNoRoom(clipId))}
      style={{ left: at - SPOT_PX / 2, width: SPOT_PX }}
      className="tl-cut-spot"
    >
      <span className="tl-cut-dot" />
    </button>
  );
});
