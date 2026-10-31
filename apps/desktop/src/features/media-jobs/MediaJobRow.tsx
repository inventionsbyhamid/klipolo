import { useLingui } from '@lingui/react/macro';
import { memo } from 'react';
import type { MediaJobInfo } from '../../bindings/MediaJobInfo';
import { Button } from '../../design/Button';
import { cn } from '../../design/cn';
import { IconButton } from '../../design/IconButton';
import { Tooltip } from '../../design/Tooltip';
import { Icon } from '../../layout/Icon';
import { becauseText, failedText, pausedText, reasonText } from '../../state/mediaJobReasons';
import { sayRefusal, useMediaJobs, wholePercent } from '../../state/mediaJobs';
import { usePressGuard } from './usePressGuard';

/** Who holds every copy (the list's `held`): the person's pause, an action's, or neither (null). */
export type HoldBy = 'person' | 'action' | null;

/**
 * One file's editing copy: its name, what is happening and why (in Rust's codes; who paused it, from the list's
 * `held`), why the file gets a copy, its progress, and Stop (or Try again once it failed or was stopped, and Dismiss
 * once it failed). Drawn again only when what it shows changes.
 */
function Row({ job, hold }: RowProps) {
  const { t } = useLingui();
  const { name, state } = job;
  const percent = wholePercent(job.progress);
  const ended = state === 'failed' || state === 'stopped';
  // A copy that waits, or is paused by a hold, while someone paused them all: who did, so a reload still says it.
  const heldBy = state === 'waiting' || job.reason?.code === 'copies.paused.held' ? hold : null;
  const text = (() => {
    switch (state) {
      case 'working':
        return t({ id: 'mediaJobs.state.working', message: 'Making an editing copy' });
      case 'stopped':
        return t({
          id: 'mediaJobs.state.stopped',
          message: 'Stopped. Press Try again to make it.',
          comment: 'The person stopped this copy',
        });
      case 'done':
        return t({ id: 'mediaJobs.state.done', message: 'Ready', comment: 'Its editing copy is made' });
    }
    if (heldBy === 'person')
      return t({
        id: 'mediaJobs.held.person',
        message: 'Paused by you',
        comment: 'The person paused editing copies (Pause all); Resume all makes them again',
      });
    if (heldBy === 'action')
      return t({
        id: 'mediaJobs.held.action',
        message: 'Paused by chat',
        comment: 'Chat (or an app connected to Klipolo) paused editing copies; only it resumes them',
      });
    if (state === 'waiting')
      return t({
        id: 'mediaJobs.state.waiting',
        message: 'Waiting its turn',
        comment: 'Waiting its turn to be copied',
      });
    // Rust's reason by its code; a code this build doesn't know is said by the state.
    return reasonText(job) ?? (state === 'paused' ? pausedText() : failedText(name));
  })();
  // Why the file gets a copy, while its copy is still to be made.
  const because = !ended && job.why ? becauseText(job.why) : null;
  const actionName = ended
    ? // The visible words start the name (WCAG 2.5.3); the file says which row's.
      t({
        id: 'mediaJobs.retry.name',
        message: `Try again to make an editing copy of ${name}`,
        comment: '{name} is a file name',
      })
    : t({
        id: 'mediaJobs.stop.name',
        message: `Stop making an editing copy of ${name}`,
        comment: '{name} is a file name; what it made so far is deleted, the file itself is untouched',
      });
  const copies = useMediaJobs.getState;
  const meant = usePressGuard(ended ? 'retry' : 'stop');
  return (
    <li data-row className="copies-row">
      <div className="copies-row-text">
        <span className="copies-row-name" title={name}>
          {name}
        </span>
        <span className="copies-row-state">{text}</span>
        {because && <span className="copies-row-why">{because}</span>}
        {!ended && (
          <div
            role="progressbar"
            aria-label={name}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent}
            className="copies-bar"
          >
            <div
              // A paused bar is drawn quieter: it isn't moving.
              className={cn('copies-bar-fill', state === 'paused' && 'is-paused')}
              style={{ width: `${percent}%` }}
            />
          </div>
        )}
      </div>
      {/* One button whatever the state (Stop while it is made, Try again once it failed or was stopped), so the
          focus stays on it when Rust's next list changes the row. */}
      <Tooltip label={actionName}>
        <Button
          size="sm"
          variant={ended ? 'secondary' : 'ghost'}
          data-row-action
          aria-label={actionName}
          className={cn(!ended && 'copies-stop')}
          onClick={() => {
            if (meant()) void (ended ? copies().retry(job.path) : copies().stop(job.path)).then(sayRefusal);
          }}
        >
          {ended ? t({ id: 'mediaJobs.retry', message: 'Try again' }) : <Icon name="x" />}
        </Button>
      </Tooltip>
      {state === 'failed' && (
        <IconButton
          icon={<Icon name="x" />}
          label={t({
            id: 'mediaJobs.dismiss.name',
            message: `Dismiss the failed copy of ${name}`,
            comment: '{name} is a file name; hides this failure until the copy fails again (nothing is deleted)',
          })}
          className="icon-btn-sm"
          onClick={() => copies().dismiss(job.path)}
        />
      )}
    </li>
  );
}

type RowProps = { job: MediaJobInfo; hold: HoldBy };

/** The fields a row shows: a progress event that moves less than a whole percent draws nothing. */
const same = (a: RowProps, b: RowProps) =>
  a.hold === b.hold &&
  a.job.path === b.job.path &&
  a.job.name === b.job.name &&
  a.job.state === b.job.state &&
  a.job.why === b.job.why &&
  a.job.reason?.code === b.job.reason?.code &&
  a.job.reason?.params.name === b.job.reason?.params.name &&
  wholePercent(a.job.progress) === wholePercent(b.job.progress);

export const MediaJobRow = memo(Row, same);
