import { useEffect, useId, useRef, useState } from 'react';
import type { ExportJobInfo } from '../../bindings/ExportJobInfo';
import { Button } from '../../design/Button';
import { useIntlLocale } from '../../i18n/react';
import { formatFileSize } from '../../lib/format';
import { canStartOver, useExport } from '../../state/export';
import { saidText } from '../../state/exportRun';
import { useProject } from '../../state/project';
import { toast } from '../../state/toasts';
import { CompatibilityButton } from './CompatibilityButton';

/** What a row says of a job whose project changed since it started. */
export const STALE = 'This project changed since the export started.';
/** What Resume does for such a job: it finishes the version that was exported. */
export const STALE_RESUME = 'Resume finishes the version from when you started.';

/** What a row says of a job another build of Klipolo made: it can't resume. */
export const OTHER_BUILD = 'Another version of Klipolo made this export, so it can’t resume.';

/**
 * "Resume export of Launch: 3 of 10 parts done", "Exporting Launch: …" while it runs, "Unfinished export of Launch: …"
 * when another build made it, and "An export Klipolo can’t read" when it can't be read.
 */
export function jobText(job: ExportJobInfo): string {
  const parts = `${job.segmentsDone} of ${job.segmentsTotal} parts done`;
  const name = job.projectName || 'an earlier project';
  if (job.state === 'running') return `Exporting ${name}: ${parts}`;
  if (!job.projectName) return 'An export Klipolo can’t read';
  if (job.otherBuild) return `Unfinished export of ${name}: ${parts}`;
  return `Resume export of ${name}: ${parts}`;
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
  const locale = useIntlLocale();
  const resumeButton = useRef<HTMLButtonElement>(null);
  /** Its Pause took: once it shows paused, the keyboard goes to its Resume. */
  const [paused, setPaused] = useState(false);
  useEffect(() => {
    if (paused && job.state !== 'running') {
      setPaused(false);
      resumeButton.current?.focus();
    }
  }, [paused, job.state]);
  const store = useExport.getState;
  const startingOver = useExport((s) => s.startingOver);
  // A yes or no, so an edit that leaves it as it was renders no row again.
  const restartable = useProject((s) => canStartOver(job, s.snapshot?.project.settings.formats ?? []));
  // An unreadable job has no project name (its reason is technical): it can only be discarded.
  const unreadable = !job.projectName && job.state !== 'running';
  // Why it paused or failed, always; a changed project or another build is a line beside it, never instead of it.
  const note = unreadable ? 'It can only be discarded.' : job.message === null ? null : saidText(job.message);
  const chosen = job.compatibility?.chosen ?? false;
  // Another build's job never resumes: Start over (this project's, with one of its formats still in it) or Discard.
  const resumable = !job.otherBuild;
  const startOver = here && restartable && (job.stale || job.otherBuild);
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
      {/* Its lines, spaced, so a screen reader reads them as sentences, not run together. */}
      <span id={id} className="ex-job-text">
        <span className="ex-job-name">{jobText(job)}</span>{' '}
        {note && <span>{note}</span>}{' '}
        {job.otherBuild && !unreadable && <span>{OTHER_BUILD}</span>}{' '}
        {job.stale && !unreadable && resumable && (
          <>
            <span>{STALE}</span> <span>{STALE_RESUME}</span>
          </>
        )}{' '}
        {job.bytes > 0 && <span>{`Uses ${formatFileSize(locale, job.bytes)}`}</span>}
      </span>
      <span className="ex-job-actions">
        {job.state === 'running' ? (
          <Button
            size="sm"
            aria-describedby={id}
            onClick={() =>
              void store()
                .pause(job.jobId)
                .then((refused) => {
                  if (refused === null) setPaused(true);
                  else toast.error(refused);
                })
            }
          >
            Pause
          </Button>
        ) : unreadable ? (
          discard
        ) : (
          <>
            {resumable && (
              <Button
                ref={resumeButton}
                size="sm"
                variant="primary"
                aria-describedby={id}
                onClick={() => void store().resume(job.jobId)}
              >
                {/* Chosen already: Resume continues in compatibility mode, so it is the one action offered. */}
                {chosen ? 'Resume in compatibility mode' : 'Resume'}
              </Button>
            )}
            {resumable && job.compatibility && !chosen && (
              <CompatibilityButton jobId={job.jobId} partsToRedo={job.compatibility.partsToRedo} describedBy={id} />
            )}
            {startOver && (
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
