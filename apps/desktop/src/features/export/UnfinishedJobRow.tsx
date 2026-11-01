import { useId } from 'react';
import type { ExportJobInfo } from '../../bindings/ExportJobInfo';
import { Button } from '../../design/Button';
import { canStartOver, useExport } from '../../state/export';
import { saidText } from '../../state/exportRun';
import { useProject } from '../../state/project';
import { toast } from '../../state/toasts';
import { CompatibilityButton } from './CompatibilityButton';

/** What a row says of a job whose project changed since it started. */
export const STALE = 'This project changed since the export started.';
/** What Resume does for such a job: it finishes the version that was exported. */
export const STALE_RESUME = 'Resume finishes the version from when you started.';

/** "Resume export of Launch: 3 of 10 parts done", or "Exporting Launch: …" while it runs. */
export function jobText(job: ExportJobInfo): string {
  const parts = `${job.segmentsDone} of ${job.segmentsTotal} parts done`;
  const name = job.projectName;
  if (job.state === 'running') return name ? `Exporting ${name}: ${parts}` : `Exporting an earlier project: ${parts}`;
  return name ? `Resume export of ${name}: ${parts}` : `Resume export of an earlier project: ${parts}`;
}

interface UnfinishedJobRowProps {
  job: ExportJobInfo;
  /** The job is of the open project: Start over can export it again. */
  here: boolean;
  /** Called before an action that removes this row (Discard, Start over), so focus can follow to the next one. */
  onLeave: () => void;
}

/**
 * One unfinished export: why it stopped, then Resume and Discard (Pause while it runs). When its project changed since,
 * Resume still finishes the version that was exported, and Start over exports the latest changes instead. After
 * repeated hardware failures it can also finish in compatibility mode.
 */
export function UnfinishedJobRow({ job, here, onLeave }: UnfinishedJobRowProps) {
  const id = useId();
  const store = useExport.getState;
  const startingOver = useExport((s) => s.startingOver);
  // A yes or no, so an edit that leaves it as it was renders no row again.
  const restartable = useProject((s) => canStartOver(job, s.snapshot?.project.settings.formats ?? []));
  // An unreadable job has no project name (its reason is technical): it can only be discarded.
  const unreadable = !job.projectName;
  // Why it paused or failed, always; a changed project is a line beside it, never instead of it.
  const note = unreadable
    ? 'This export can’t be read, so it can only be discarded.'
    : job.message === null
      ? null
      : saidText(job.message);
  const chosen = job.compatibility?.chosen ?? false;
  const discard = (
    <Button
      size="sm"
      aria-describedby={id}
      onClick={() => {
        onLeave();
        void store().discard(job.jobId);
      }}
    >
      Discard
    </Button>
  );
  return (
    <li className="ex-job">
      <span id={id} className="ex-job-text">
        <span className="ex-job-name">{jobText(job)}</span>
        {note && <span>{note}</span>}
        {job.stale && !unreadable && (
          <>
            <span>{STALE}</span>
            <span>{STALE_RESUME}</span>
          </>
        )}
      </span>
      <span className="ex-job-actions">
        {job.state === 'running' ? (
          <Button
            size="sm"
            aria-describedby={id}
            onClick={() =>
              void store()
                .pause(job.jobId)
                .then((refused) => refused && toast.error(refused))
            }
          >
            Pause
          </Button>
        ) : unreadable ? (
          discard
        ) : (
          <>
            <Button size="sm" variant="primary" aria-describedby={id} onClick={() => void store().resume(job.jobId)}>
              {/* Chosen already: Resume continues in compatibility mode, so it is the one action offered. */}
              {chosen ? 'Resume in compatibility mode' : 'Resume'}
            </Button>
            {job.compatibility && !chosen && (
              <CompatibilityButton jobId={job.jobId} partsToRedo={job.compatibility.partsToRedo} />
            )}
            {job.stale && here && restartable && (
              <Button
                size="sm"
                aria-describedby={id}
                aria-disabled={startingOver || undefined}
                onClick={() => {
                  if (startingOver) return;
                  onLeave();
                  void store().startOver(job);
                }}
              >
                Start over with your latest changes
              </Button>
            )}
            {discard}
          </>
        )}
      </span>
    </li>
  );
}
