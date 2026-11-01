import { plural } from '@lingui/core/macro';
import { useLingui } from '@lingui/react/macro';
import { memo, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { cn } from '../../design/cn';
import { Popover, PopoverContent, PopoverTrigger } from '../../design/Popover';
import { useIntlLocale } from '../../i18n/react';
import { Icon } from '../../layout/Icon';
import { formatPercent } from '../../lib/format';
import { activeCount, activePercent, failedCount, useMediaJobs } from '../../state/mediaJobs';
import { useUi } from '../../state/ui';
import { COPIES_TITLE } from './AutoCopiesSwitch';
import { MediaJobsPopover } from './MediaJobsPopover';
import './mediaJobs.css';

/** The ring's radius and length (px) in its 14 px box, with a 2 px stroke. */
const R = 5.5;
const LENGTH = 2 * Math.PI * R;

/**
 * A 14 px progress ring (decorative: the pill's name says the percentage). Paper on the progress track.
 */
export function Ring({ percent }: { percent: number }) {
  return (
    <svg width={14} height={14} viewBox="0 0 14 14" aria-hidden className="copies-ring">
      <circle cx={7} cy={7} r={R} className="copies-ring-track" />
      <circle
        cx={7}
        cy={7}
        r={R}
        className="copies-ring-fill"
        strokeDasharray={LENGTH}
        strokeDashoffset={LENGTH * (1 - percent / 100)}
      />
    </svg>
  );
}

/** Focus is nowhere (the element that had it went). */
const focusLost = () => !document.activeElement || document.activeElement === document.body;

/**
 * A quiet pill in the top bar, there only while editing copies are being made, wait their turn or are paused
 * ("Preparing 2 files"), or while a copy that failed hasn't been tried again or dismissed ("Copy failed", with no
 * ring). Copies the person stopped never keep it. It opens the list of files. It subscribes to three numbers, so
 * progress that moves less than a percent draws nothing; with nothing to show it renders nothing at all. Kept while its
 * popover is open, so stopping the last copy leaves the person where they were.
 *
 * When it goes while it, or its popover, had the keyboard focus, the focus moves to `focusAfter` (the top bar's next
 * control), never to the page.
 */
function Pill({ focusAfter }: { focusAfter: RefObject<HTMLElement | null> }) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  /** The trigger has the focus (the popover closed): it goes to `focusAfter` if the pill goes. */
  const focused = useRef(false);
  /** The popover closed because of a click (or focus) outside it: the focus stays where that put it. */
  const outside = useRef(false);
  const auto = useUi((s) => s.autoCopies);
  const count = useMediaJobs((s) => activeCount(s.jobs, auto));
  const percent = useMediaJobs((s) => activePercent(s.jobs, auto));
  const failed = useMediaJobs((s) => failedCount(s.jobs, s.dismissed, auto));
  const shown = count > 0 || failed > 0 || open;
  useLayoutEffect(() => {
    if (shown || !focused.current) return;
    focused.current = false;
    if (focusLost()) focusAfter.current?.focus();
  }, [shown, focusAfter]);
  if (!shown) return null;
  const progress = formatPercent(locale, percent / 100);
  const text =
    count > 0
      ? t({
          id: 'mediaJobs.pill',
          message: plural(count, { one: 'Preparing # file', other: 'Preparing # files' }),
          comment: 'Editing copies being made, waiting or paused (a quiet pill in the top bar)',
        })
      : failed > 0
        ? t({
            id: 'mediaJobs.pill.failed',
            message: plural(failed, { one: 'Copy failed', other: '# copies failed' }),
            comment: "Editing copies that couldn't be made (a quiet pill in the top bar; it opens the list with Try again)",
          })
        : t(COPIES_TITLE);
  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        if (next) outside.current = false;
        setOpen(next);
      }}
    >
      <PopoverTrigger asChild>
        <button
          ref={trigger}
          type="button"
          // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
          tabIndex={0}
          data-media-jobs-pill
          className={cn('copies-pill', count === 0 && 'is-quiet')}
          // The visible words start the name (WCAG 2.5.3); the percentage follows.
          aria-label={
            count === 0
              ? undefined
              : t({
                  id: 'mediaJobs.pill.name',
                  message: plural(count, {
                    one: `Preparing # file, ${progress}`,
                    other: `Preparing # files, ${progress}`,
                  }),
                  comment: 'Editing copies being made; {progress} is their mean progress as a percentage, e.g. 40%',
                })
          }
          title={t({
            id: 'mediaJobs.pill.title',
            message: 'Editing copies help the preview play smoothly',
          })}
          onFocus={() => {
            focused.current = true;
          }}
          onBlur={(event) => {
            // Focus moved elsewhere (into the popover, or another control); a blur as the pill goes keeps it.
            if (event.relatedTarget || trigger.current?.isConnected) focused.current = false;
          }}
        >
          {count > 0 ? <Ring percent={percent} /> : failed > 0 && <Icon name="info" />}
          {text}
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        aria-label={t(COPIES_TITLE)}
        className="copies-popover"
        onInteractOutside={() => {
          outside.current = true;
        }}
        onCloseAutoFocus={(event) => {
          // A click outside (the timeline, the preview) leaves the focus where it put it, even on the page, so Space
          // still plays: Radix does that once the trigger is gone. Closed from the keyboard (Escape) with nothing left
          // to show, the pill is gone: the focus goes on to the top bar's next control, not the page.
          const fromOutside = outside.current;
          outside.current = false;
          if (fromOutside || trigger.current?.isConnected) return;
          event.preventDefault();
          if (focusLost()) focusAfter.current?.focus();
        }}
      >
        <MediaJobsPopover />
      </PopoverContent>
    </Popover>
  );
}

/** Its one prop is a stable ref: the top bar drawing again never draws it. */
export const MediaJobsPill = memo(Pill);
