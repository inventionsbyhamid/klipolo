import { useLingui } from '@lingui/react/macro';
import { memo, useCallback, type MouseEvent, type PointerEvent } from 'react';
import { cn } from '../../../design/cn';
import { useIntlLocale } from '../../../i18n/react';
import { Icon } from '../../../layout/Icon';
import { formatSeconds } from '../../../lib/format';
import { countRender } from '../../../lib/renderCount';
import { splitDuration } from '../../../lib/transitions';
import { registerBlock } from '../drag/registry';
import { cutId } from '../keys/laneOrder';
import { addTip, transitionLabels, type FadesReplaced } from '../keys/transitionActions';
import { usToPx } from '../scale';

/** A transition's chip is never narrower than this (px), so a short one zoomed out can still be clicked. */
export const CHIP_NARROWEST_PX = 14;
/** A chip this wide (px) has room for its icon. */
const CHIP_ICON_PX = 18;
/** A bare cut's spot (px): its Add transition button or note, as wide as a fingertip's aim needs. */
const SPOT_PX = 20;

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
}

/** A press takes focus, as a block's does (WebKit doesn't focus buttons on click), and never reaches the lane. */
function pressMarker(e: PointerEvent<HTMLElement>) {
  if (e.button !== 0) return;
  e.preventDefault();
  e.stopPropagation();
  e.currentTarget.focus({ preventScroll: true });
}

/**
 * A cut on the Video lane, along the lane's bottom edge, clear of the clips' names at the top and of the trim
 * grips in the middle of their edges, so a press on a clip's edge there still trims.
 * - Its transition: a chip centred on the cut and as wide as it lasts.
 * - Else an "Add transition" button, shown only while its spot is pointed at or has focus.
 * - Else, where neither clip has footage to spare, a note saying so, shown the same way.
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
}: CutMarkerProps) {
  const id = cutId(clipId);
  countRender(id);
  const { t } = useLingui();
  const locale = useIntlLocale();
  const ref = useCallback((el: HTMLElement | null) => registerBlock(id, el), [id]);
  const at = usToPx(cutUs, pps);
  const common = {
    ref,
    'data-block-id': id,
    tabIndex: tabbable ? 0 : -1,
    onPointerDown: pressMarker,
  };
  /** A click (a keyboard's Enter is the lane's: `useRovingFocus`) acts, and never seeks the lane. */
  const onClick = (act: () => void) => (e: MouseEvent) => {
    e.stopPropagation();
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
