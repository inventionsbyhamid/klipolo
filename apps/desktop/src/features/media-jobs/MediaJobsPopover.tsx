import { useLingui } from '@lingui/react/macro';
import { useLayoutEffect, useRef, type FocusEvent, type RefObject } from 'react';
import { Button } from '../../design/Button';
import { copyWhy, isListed, pausedForAWhile, sayRefusal, useMediaJobs } from '../../state/mediaJobs';
import { useUi } from '../../state/ui';
import { ABOUT_COPIES, AutoCopiesSwitch, COPIES_TITLE } from './AutoCopiesSwitch';
import { MediaJobRow, type HoldBy } from './MediaJobRow';
import { usePressGuard } from './usePressGuard';

/**
 * Keeps keyboard focus in the popover when the control that had it goes (a dismissed or finished row, a footer with
 * nothing left to pause): it moves to the row now in that place, else the last row, else the footer's button, else
 * the switch. Never to the page: in WebKit the next Tab would start outside the popover and close it.
 */
function useKeepFocus(root: RefObject<HTMLDivElement | null>) {
  /** The row (its place in the list) or other control focus was last in; undefined until focus comes in. */
  const last = useRef<number | null | undefined>(undefined);
  useLayoutEffect(() => {
    const el = root.current;
    if (!el || last.current === undefined) return;
    const active = el.ownerDocument.activeElement;
    if (active && active !== el.ownerDocument.body) return;
    const actions = el.querySelectorAll<HTMLElement>('[data-row-action]');
    const row = last.current;
    const target =
      (row !== null && actions.length > 0 ? actions[Math.min(row, actions.length - 1)] : null) ??
      el.querySelector<HTMLElement>('[data-footer-action]') ??
      el.querySelector<HTMLElement>('[role="switch"]');
    target?.focus();
  });
  return (event: FocusEvent<HTMLDivElement>) => {
    const row = (event.target as HTMLElement).closest('[data-row]');
    last.current = row ? [...event.currentTarget.querySelectorAll('[data-row]')].indexOf(row) : null;
  };
}

/**
 * The pill's popover: what editing copies are, the automatic switch, each file with what is happening and why, and
 * Pause all or Resume all. Mounted only while open, so it follows the list only then.
 */
export function MediaJobsPopover() {
  const { t } = useLingui();
  const root = useRef<HTMLDivElement>(null);
  const onFocus = useKeepFocus(root);
  const auto = useUi((s) => s.autoCopies);
  const jobs = useMediaJobs((s) => s.jobs);
  const dismissed = useMediaJobs((s) => s.dismissed);
  const pausedByPerson = useMediaJobs((s) => s.pausedByPerson);
  const pausedByAction = useMediaJobs((s) => s.pausedByAction);
  const rows = jobs.filter((job) => isListed(job, auto, dismissed));
  // Who holds every copy (Rust's `held`, so it holds after a reload too): the person's own pause first, the one
  // Resume all lifts.
  const hold: HoldBy = pausedByPerson ? 'person' : pausedByAction ? 'action' : null;
  // Pause all while a copy is being made or waits its turn (or only waits for playback or an export to end).
  const canPause =
    !pausedByPerson &&
    jobs.some((job) => job.state === 'working' || (job.state === 'waiting' && auto) || pausedForAWhile(job));
  // Resume all once the person paused, or a copy waits for them (a full disk; heat and a busy encoder are also tried
  // again by themselves). A copy held only by chat's pause isn't theirs to resume: Resume all wouldn't start it.
  const canResume =
    pausedByPerson ||
    jobs.some(
      (job) =>
        job.state === 'paused' && !pausedForAWhile(job) && !(pausedByAction && copyWhy(job) === 'held'),
    );
  // Copies that only Resume all starts again: the person's own pause, a full disk, or a hold this window can't place
  // (a reload forgot whose); heat and a busy encoder are tried again by themselves.
  const waitsForPerson =
    pausedByPerson ||
    jobs.some((job) => {
      if (job.state !== 'paused') return false;
      const why = copyWhy(job);
      return why === 'diskFull' || why === null || (why === 'held' && !pausedByAction);
    });
  // One button for both, so the focus stays on it when it turns from Pause all to Resume all. When both apply, Pause
  // all while a copy is being made or nothing waits for the person (so a retry after heat can still be held off);
  // otherwise Resume all.
  const working = jobs.some((job) => job.state === 'working');
  const footer =
    canPause && (working || !waitsForPerson) ? 'pause' : canResume ? 'resume' : canPause ? 'pause' : null;
  const meant = usePressGuard(footer ?? '');
  const store = useMediaJobs.getState;
  return (
    <div ref={root} onFocus={onFocus} className="copies-pop">
      <div className="copies-head">
        <h2>{t(COPIES_TITLE)}</h2>
        <p>{t(ABOUT_COPIES)}</p>
      </div>
      <AutoCopiesSwitch />
      {rows.length > 0 ? (
        <ul className="copies-rows">
          {rows.map((job) => (
            <MediaJobRow key={job.path} job={job} hold={hold} />
          ))}
        </ul>
      ) : (
        <p className="copies-none">
          {auto
            ? t({
                id: 'mediaJobs.none',
                message: 'No files are being prepared. Large videos you add get a copy by themselves.',
              })
            : t({
                id: 'mediaJobs.none.off',
                message: 'Automatic copies are off. Turn them on to make copies of large videos.',
              })}
        </p>
      )}
      {pausedByAction && (
        <p className="copies-note">
          {t({
            id: 'mediaJobs.pausedByAction',
            message: 'Paused by chat. Ask chat to resume editing copies.',
            comment:
              "Chat (or an app connected to Klipolo) paused copies; Resume all here doesn't undo a pause chat asked for",
          })}
        </p>
      )}
      {footer && (
        <div className="copies-foot">
          <Button
            size="sm"
            data-footer-action
            onClick={() => {
              if (meant())
                void (footer === 'resume' ? store().resume('person') : store().pause('person')).then(
                  sayRefusal,
                );
            }}
          >
            {footer === 'resume'
              ? t({ id: 'mediaJobs.resumeAll', message: 'Resume all' })
              : t({ id: 'mediaJobs.pauseAll', message: 'Pause all' })}
          </Button>
        </div>
      )}
    </div>
  );
}
