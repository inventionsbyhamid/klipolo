import { type KeyboardEvent, type ReactNode, useCallback, useEffect, useRef, useState } from 'react';
import { useShallow } from 'zustand/react/shallow';
import type { ExportPlan } from '../../bindings/ExportPlan';
import type { FormatKind } from '../../bindings/FormatKind';
import { exporter } from '../../bridge/exporter';
import { Button } from '../../design/Button';
import { useIntlLocale } from '../../i18n/react';
import { Icon } from '../../layout/Icon';
import { formatFileSize, formatPercent } from '../../lib/format';
import { FORMAT_SHORT } from '../../lib/layout';
import { videoDurationUs } from '../../lib/timeline';
import { formatTime } from '../../player/time';
import { stateOf, useAnalysis } from '../../state/analysis';
import { canStartOver, chosenCaptionFile, exportPercent, shownJob, useExport } from '../../state/export';
import { failedText } from '../../state/exportRun';
import { useProject } from '../../state/project';
import { toast } from '../../state/toasts';
import { CaptionFileChoice } from './CaptionFileChoice';
import { CompatibilityButton } from './CompatibilityButton';
import { ExportFiles } from './ExportFiles';
import { ExportOutcome } from './ExportOutcome';
import { ExportProgress } from './ExportProgress';
import { FormatCards } from './FormatCards';
import { ExportIcon } from './icons';
import { FORMATS, roughly, videos } from './presets';
import { QualityPicker } from './QualityPicker';
import { STALE, STALE_RESUME } from './UnfinishedJobRow';
import { UnfinishedJobs } from './UnfinishedJobs';
import './export.css';

const FOCUSABLE = 'button:not(:disabled), [tabindex="0"]';

type View = 'choose' | 'running' | 'paused' | 'failed' | 'done';

/**
 * The export dialog: the unfinished exports to resume or discard, then pick the
 * formats and the quality and where the files go, then export; while it runs, one bar and Cancel export; when it is
 * done, the files with Open and Show in Finder; when it pauses (a full disk, a busy encoder) or fails, why and Resume or
 * Try again, which go on from its finished parts, or Discard. Mounted once; opens from the top bar's Export or Command
 * E. Esc and the close button close it, and focus goes back where it was; an export still running carries on behind it
 * (the top bar shows its progress), as only Cancel export stops one.
 */
export function ExportDialog() {
  const open = useExport((s) => s.open);
  const project = useProject((s) => s.snapshot !== null);
  return open && project ? <Dialog /> : null;
}

function viewOf(phase: string): View {
  if (phase === 'running') return 'running';
  if (phase === 'paused') return 'paused';
  if (phase === 'done') return 'done';
  if (phase === 'failed') return 'failed';
  return 'choose';
}

function Dialog() {
  const view = useExport((s) => viewOf(s.phase));
  const announcement = useExport((s) => s.announcement);
  const failed = useExport((s) => s.phase === 'failed');
  const root = useRef<HTMLDivElement>(null);
  const close = () => useExport.getState().setOpen(false);

  useEffect(() => {
    const dialog = root.current;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    // The dialog is modal: no key reaches the editor behind it, and focus that leaves it comes back.
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        close();
      } else if (dialog && !dialog.contains(e.target as Node)) {
        e.preventDefault();
        e.stopPropagation();
        primary(dialog)?.focus();
      }
    };
    const onFocus = (e: FocusEvent) => {
      if (dialog && !dialog.contains(e.target as Node)) primary(dialog)?.focus();
    };
    window.addEventListener('keydown', onKey, true);
    document.addEventListener('focusin', onFocus);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      document.removeEventListener('focusin', onFocus);
      trigger?.focus();
    };
  }, []);

  // On opening and on every view change, the view's primary control: the one that had focus may be gone.
  useEffect(() => {
    if (root.current) primary(root.current)?.focus();
  }, [view]);

  // Keys typed in the dialog stay in it (the editor's shortcuts listen on the window).
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    e.stopPropagation();
    if (e.key === 'Tab') trapFocus(e, root.current);
  };

  return (
    <div className="scrim export-scrim">
      <div
        className="dialog export-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="export-title"
        ref={root}
        onKeyDown={onKeyDown}
      >
        {view === 'done' ? <ExportOutcome onClose={close} /> : <Working view={view} onClose={close} />}
        {/* Mounted with the dialog, so what they say next is announced: progress and results as a status, a failure as
            an alert. */}
        <p role="status" className="sr-only">
          {failed ? '' : announcement}
        </p>
        <p role="alert" className="sr-only">
          {failed ? announcement : ''}
        </p>
      </div>
    </div>
  );
}

/** The view's primary control (`data-primary`), else its title. */
function primary(root: HTMLElement): HTMLElement | null {
  return root.querySelector<HTMLElement>('[data-primary]') ?? root.querySelector<HTMLElement>('h2');
}

function trapFocus(e: KeyboardEvent<HTMLDivElement>, root: HTMLElement | null) {
  const items = [...(root?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])];
  if (items.length === 0) return;
  const [first, last] = [items[0], items[items.length - 1]];
  if (e.shiftKey && document.activeElement === first) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && document.activeElement === last) {
    e.preventDefault();
    first.focus();
  }
}

/** The formats, then the choices (f0), the progress (f1) or what went wrong (failed). */
function Working({ view, onClose }: { view: Exclude<View, 'done'>; onClose: () => void }) {
  const locale = useIntlLocale();
  const { name, durationUs, primaryFormat } = useProject(
    useShallow((s) => ({
      name: s.snapshot?.project.name ?? '',
      durationUs: s.snapshot ? videoDurationUs(s.snapshot.project.timeline) : 0,
      primaryFormat: s.snapshot?.project.settings.primaryFormat ?? 'landscape',
    })),
  );
  const run = useExport(useShallow((s) => ({ files: s.files, formats: s.formats, picked: s.picked })));
  const quality = useExport((s) => s.quality);
  const folder = useExport((s) => s.folder);
  const [plan, setPlan] = useState<ExportPlan | null>(null);
  const making = useCaptionsBeingMade();
  // A clip's analysis finished: its words may be new, so the plan (a captions file has words to save) is asked again.
  const heard = useAnalysis((s) => Object.values(s.states).filter((a) => a.state === 'done').length);
  const checked: FormatKind[] =
    view === 'choose'
      ? FORMATS.filter((f) => (run.picked ?? [primaryFormat]).includes(f))
      : run.files.length > 0
        ? run.files.map((f) => f.format)
        : run.formats;
  const key = checked.join();
  // "Also save captions as a file": once some clip's words are heard, beside a 16:9 video.
  const offerCaptions = view === 'choose' && !!plan?.canSaveCaptions && checked.includes('landscape');

  useEffect(() => {
    if (view !== 'choose') return;
    let current = true;
    const formats = key ? (key.split(',') as FormatKind[]) : [];
    exporter.plan(formats, quality).then(
      (next) => {
        // Where the files go is known now, for the finished dialog too, whatever the view does meanwhile.
        useExport.getState().setFolder(next.folder);
        if (current) setPlan(next);
      },
      () => current && setPlan(null),
    );
    return () => {
      current = false;
    };
  }, [view, key, quality, folder, heard]);

  // A clip's words came in: whether an unfinished export's project changed is asked again.
  const firstHeard = useRef(heard);
  useEffect(() => {
    if (view !== 'choose' || heard === firstHeard.current) return;
    firstHeard.current = heard;
    void useExport.getState().loadJobs();
  }, [view, heard]);

  // The last unfinished row went: focus goes to the view's primary control.
  const onEmptied = useCallback(
    () => document.querySelector<HTMLElement>('.export-dialog [data-primary]')?.focus(),
    [],
  );

  const toggle = (format: FormatKind) =>
    useExport.getState().setPicked(checked.includes(format) ? checked.filter((f) => f !== format) : [...checked, format]);

  return (
    <>
      <div className="dialog-head">
        <div>
          <h2 className="dialog-title" id="export-title" tabIndex={-1}>
            Export
          </h2>
          <p className="dialog-sub">
            {name}, <span className="ex-tabular">{formatTime(durationUs)}</span>
          </p>
        </div>
        <button type="button" className="icon-btn" aria-label="Close" title="Close (Esc)" onClick={onClose}>
          <ExportIcon name="x" />
        </button>
      </div>
      {view === 'choose' && <UnfinishedJobs onEmptied={onEmptied} />}
      <FormatCards
        checked={checked}
        files={view === 'choose' ? (plan?.files ?? []) : run.files}
        locked={view !== 'choose'}
        onToggle={toggle}
      />
      {view === 'choose' && (
        <Options
          folder={plan?.folder ?? folder}
          slowMaster={plan?.slowMaster ?? false}
          maximumAsH264={plan?.maximumAsH264 ?? false}
          captions={offerCaptions}
        />
      )}
      {view === 'choose' && making > 0 && <CaptionsNote count={making} />}
      {view === 'running' && <ExportProgress />}
      {view === 'paused' && <Paused />}
      {view === 'failed' && <Failed />}
      {(view === 'paused' || view === 'failed') && <KeptJobActions />}
      {view === 'choose' && (
        <ChooseFoot
          count={checked.length}
          bytes={plan?.bytes ?? null}
          seconds={plan?.seconds ?? 0}
          formats={checked}
          onClose={onClose}
          locale={locale}
          captions={making > 0}
          // Before the first plan, the kept choice goes as it is: Export pressed early never drops it.
          offersCaptionFile={plan === null || offerCaptions}
        />
      )}
      {view === 'running' && <RunningFoot />}
      {view === 'paused' && <PausedFoot onClose={onClose} />}
      {view === 'failed' && <FailedFoot onClose={onClose} />}
    </>
  );
}

/**
 * How many clips in the video still have their captions being made (their analysis hasn't timed its words yet), while
 * captions are on: an export now freezes only finished transcripts, so their captions are left out.
 */
function useCaptionsBeingMade(): number {
  const clips = useProject(
    useShallow((s) => {
      const project = s.snapshot?.project;
      if (!project?.captions.enabled) return [];
      const ids = project.timeline.main.flatMap((c) => (c.source.type === 'asset' ? [c.source.id] : []));
      return [...new Set(ids)];
    }),
  );
  return useAnalysis((s) =>
    clips.reduce((n, id) => {
      const state = stateOf(s.states, id);
      return state.state === 'running' && state.step !== 'scenes' ? n + 1 : n;
    }, 0),
  );
}

/** "Captions are still being made for 2 clips. …", plainly; the export still goes. */
function CaptionsNote({ count }: { count: number }) {
  return (
    <p className="ex-note" id="export-captions-note">
      <Icon name="info" />
      <span>
        {count > 1
          ? `Captions are still being made for ${count} clips. Exporting now leaves their captions out.`
          : 'Captions are still being made for 1 clip. Exporting now leaves its captions out.'}
      </span>
    </p>
  );
}

/**
 * Quality (what it is for under it; `slowMaster`: Master is slower here; `maximumAsH264`: Maximum saves H.264 here), the
 * captions file once `captions` offers it,
 * and where the files go.
 */
function Options({
  folder,
  slowMaster,
  maximumAsH264,
  captions,
}: {
  folder: string | null;
  slowMaster: boolean;
  maximumAsH264: boolean;
  captions: boolean;
}) {
  return (
    <dl className="ex-opts">
      <dt className="ex-dt-top">Quality</dt>
      <dd>
        <QualityPicker slowMaster={slowMaster} maximumAsH264={maximumAsH264} />
      </dd>
      {captions && (
        <>
          <dt>Captions</dt>
          <dd>
            <CaptionFileChoice />
          </dd>
        </>
      )}
      <dt>Save to</dt>
      <dd>
        <span className="ex-path">
          <ExportIcon name="folder" />
          {folder ?? 'Movies / Klipolo'}
        </span>
        <Button
          variant="ghost"
          size="sm"
          aria-label="Change folder"
          onClick={() => void useExport.getState().pickFolder()}
        >
          Change
        </Button>
      </dd>
    </dl>
  );
}

/** "2 videos, about 280 MB", Cancel and "Export 2 videos" (off, saying why, with no format picked). */
function ChooseFoot({
  count,
  bytes,
  seconds,
  formats,
  onClose,
  locale,
  captions,
  offersCaptionFile,
}: {
  count: number;
  bytes: number | null;
  /** About how long it takes on this computer (the plan's), 0 while unknown. */
  seconds: number;
  formats: FormatKind[];
  onClose: () => void;
  locale: string;
  /** The captions note is shown: Export is described by it. */
  captions: boolean;
  /** "Also save captions as a file" is offered: the file goes with the export when it is on. */
  offersCaptionFile: boolean;
}) {
  const none = count === 0;
  const stopped = useExport((s) => s.phase === 'cancelled');
  const summary = none
    ? 'Pick at least one format'
    : bytes !== null && bytes > 0
      ? sized(videos(count), formatFileSize(locale, bytes), seconds)
      : videos(count);
  return (
    <div className="dialog-foot">
      <span className="left" id="export-summary">
        {stopped && !none ? 'Stopped. Nothing was saved.' : summary}
      </span>
      <Button onClick={onClose}>Cancel</Button>
      <Button
        variant="primary"
        data-primary
        aria-disabled={none || undefined}
        aria-describedby={none ? 'export-summary' : captions ? 'export-captions-note' : undefined}
        onClick={() => {
          if (none) return;
          const captionFile = chosenCaptionFile(formats, offersCaptionFile);
          void useExport.getState().start(formats, undefined, { captionFile });
        }}
      >
        <Icon name="export" />
        {none ? 'Export' : `Export ${videos(count)}`}
      </Button>
    </div>
  );
}

/**
 * The formats already in place, Cancel export, Pause, and the primary turned "Exporting" with a spinner. Cancel export
 * deletes the finished parts, so once there are some it asks first, in place: Keep paused (Keep exporting while it
 * finishes, which can't pause) or Delete.
 */
function RunningFoot() {
  const done = useExport(
    useShallow((s) => s.files.filter((_, i) => s.ready.includes(i)).map((f) => FORMAT_SHORT[f.format])),
  );
  const { stopping, pausing, finishing, starting, kept } = useExport(
    useShallow((s) => ({
      stopping: s.stopping,
      pausing: s.pausing,
      finishing: s.finishing,
      starting: !s.jobId,
      kept: s.segmentsDone > 0,
    })),
  );
  const [asking, setAsking] = useState(false);
  const pauseButton = useRef<HTMLButtonElement>(null);
  const keepButton = useRef<HTMLButtonElement>(null);
  const asked = useRef(false);
  useEffect(() => {
    // The question takes the keyboard on Keep; once it's gone, focus goes back to Pause.
    if (asking) keepButton.current?.focus();
    else if (asked.current) pauseButton.current?.focus();
    asked.current = asking;
  }, [asking]);
  const cancel = () =>
    void useExport
      .getState()
      .cancel()
      .then((refused) => {
        if (refused !== null) toast.error(refused);
      });
  const pause = () =>
    void useExport
      .getState()
      .pause()
      .then((refused) => {
        if (refused !== null) toast.error(refused);
      });
  if (asking) {
    return (
      <div className="dialog-foot" role="group" aria-labelledby="export-stop-ask">
        <span className="left ex-ask" id="export-stop-ask">
          Stop and delete the finished parts?
        </span>
        <Button
          onClick={() => {
            setAsking(false);
            cancel();
          }}
        >
          Delete
        </Button>
        <Button
          ref={keepButton}
          variant="primary"
          onClick={() => {
            setAsking(false);
            // Joining the parts can't pause: keeping it means letting it finish.
            if (!finishing) pause();
          }}
        >
          {finishing ? 'Keep exporting' : 'Keep paused'}
        </Button>
      </div>
    );
  }
  const still = pausing || stopping || finishing;
  return (
    <div className="dialog-foot">
      <span className="left">
        {done.length > 0 && (
          <span className="ex-done">
            <Icon name="check" />
            {done.length > 1 ? `${done.join(' and ')} are done` : `${done[0]} is done`}
          </span>
        )}
      </span>
      <Button
        data-primary={starting || undefined}
        aria-disabled={stopping || undefined}
        onClick={() => {
          if (stopping) return;
          if (kept) setAsking(true);
          else cancel();
        }}
      >
        Cancel export
      </Button>
      {!starting && (
        <Button
          ref={pauseButton}
          data-primary
          aria-disabled={still || undefined}
          aria-describedby={finishing ? 'export-no-pause' : undefined}
          onClick={() => !still && pause()}
        >
          Pause
        </Button>
      )}
      <Button variant="primary" disabled>
        <span className="ex-spinner" aria-hidden />
        Exporting
      </Button>
    </div>
  );
}

/**
 * A paused export (Pause, Quit, a full disk, a busy encoder): how far it came, why it paused, and that what it finished
 * is kept; the files in place stay listed. Calm, not a failure: Resume goes on from there.
 */
function Paused() {
  const locale = useIntlLocale();
  const message = useExport((s) => s.message);
  const percent = useExport(exportPercent);
  return (
    <>
      <div className="ex-progress">
        <div className="ex-row">
          <span className="ex-title">Export paused</span>
          <span className="ex-pct">{formatPercent(locale, percent / 100)}</span>
        </div>
        <div
          className="ex-bar"
          role="progressbar"
          aria-label="Export progress"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
        >
          <i style={{ width: `${percent}%` }} />
        </div>
        {/* Why it paused comes first, changed project or not: Resume may meet the same cause. */}
        {message ? (
          <p className="ex-failed">
            <Icon name="info" />
            <span>{message}</span>
          </p>
        ) : (
          <p className="ex-meta">Everything finished so far is kept.</p>
        )}
        <StaleNote />
      </div>
      <ExportFiles />
    </>
  );
}

/** What went wrong, and which formats weren't saved (failed); the files in place stay listed. */
function Failed() {
  const message = useExport((s) => s.message);
  const lost = useExport(
    useShallow((s) => s.files.filter((_, i) => !s.ready.includes(i)).map((f) => f.format)),
  );
  const said = failedText(message ?? '', lost);
  return (
    <>
      <div className="ex-progress">
        <p className="ex-failed">
          <Icon name="info" />
          <span>{said}</span>
        </p>
        <StaleNote />
      </div>
      <ExportFiles />
    </>
  );
}

/** The kept job's project changed since it started: Resume still finishes the version that was exported. */
function StaleNote() {
  const stale = useExport((s) => shownJob(s)?.stale ?? false);
  if (!stale) return null;
  return (
    <>
      <p className="ex-meta">{STALE}</p>
      <p className="ex-meta">{STALE_RESUME}</p>
    </>
  );
}

/**
 * The kept job's other ways on: finishing in compatibility mode after repeated hardware failures, and, when its project
 * changed since, Start over with the latest changes (this project's job, with one of its formats still in it).
 */
function KeptJobActions() {
  const job = useExport(shownJob);
  const startingOver = useExport((s) => s.startingOver);
  const here = useProject((p) => p.snapshot?.project.id) === job?.projectId;
  const restartable = useProject((p) => !!job && canStartOver(job, p.snapshot?.project.settings.formats ?? []));
  if (!job) return null;
  const compatible = job.compatibility !== null && !job.compatibility.chosen;
  const startOver = job.stale && here && restartable;
  if (!compatible && !startOver) return null;
  return (
    <div className="ex-job-actions">
      {job.compatibility && compatible && (
        <CompatibilityButton jobId={job.jobId} partsToRedo={job.compatibility.partsToRedo} />
      )}
      {startOver && (
        <Button
          size="sm"
          aria-disabled={startingOver || undefined}
          onClick={() => !startingOver && void useExport.getState().startOver(job)}
        >
          Start over with your latest changes
        </Button>
      )}
    </div>
  );
}

/**
 * A kept job's foot: Discard, Close and `primary` (Resume, Try again; none when it can't go on). Discard deletes every
 * finished part, so it asks first, in place: "Delete the finished parts?", Keep or Delete.
 */
function KeptFoot({ onClose, primary }: { onClose: () => void; primary: ReactNode }) {
  const jobId = useExport((s) => s.jobId);
  const [asking, setAsking] = useState(false);
  const discardButton = useRef<HTMLButtonElement>(null);
  const keepButton = useRef<HTMLButtonElement>(null);
  const kept = useRef(false);
  useEffect(() => {
    // The question takes the keyboard on Keep; once it's gone after Keep, focus goes back to Discard.
    if (asking) keepButton.current?.focus();
    else if (kept.current) {
      kept.current = false;
      discardButton.current?.focus();
    }
  }, [asking]);
  if (asking && jobId) {
    return (
      <div className="dialog-foot" role="group" aria-labelledby="export-discard-ask">
        <span className="left ex-ask" id="export-discard-ask">
          Delete the finished parts?
        </span>
        <Button
          onClick={() => {
            setAsking(false);
            void useExport.getState().discard(jobId);
          }}
        >
          Delete
        </Button>
        <Button
          ref={keepButton}
          variant="primary"
          onClick={() => {
            kept.current = true;
            setAsking(false);
          }}
        >
          Keep
        </Button>
      </div>
    );
  }
  return (
    <div className="dialog-foot">
      {jobId && (
        <Button ref={discardButton} onClick={() => setAsking(true)}>
          Discard
        </Button>
      )}
      <Button onClick={onClose}>Close</Button>
      {primary}
    </div>
  );
}

/** Discard, Close, and Resume (in compatibility mode, when the person chose it). */
function PausedFoot({ onClose }: { onClose: () => void }) {
  const chosen = useExport((s) => shownJob(s)?.compatibility?.chosen ?? false);
  return (
    <KeptFoot
      onClose={onClose}
      primary={
        <Button variant="primary" data-primary onClick={() => void useExport.getState().retry()}>
          <Icon name="export" />
          {chosen ? 'Resume in compatibility mode' : 'Resume'}
        </Button>
      }
    />
  );
}

/** Discard, Close, and Try again: it goes on from the kept job's finished parts. */
function FailedFoot({ onClose }: { onClose: () => void }) {
  return (
    <KeptFoot
      onClose={onClose}
      primary={
        <Button variant="primary" data-primary onClick={() => void useExport.getState().retry()}>
          <Icon name="export" />
          Try again
        </Button>
      }
    />
  );
}

/** "2 videos, about 280 MB. Takes about 4 minutes.", or without the time while it is unknown. */
function sized(count: string, size: string, seconds: number): string {
  return seconds > 0 ? `${count}, about ${size}. Takes ${roughly(seconds)}.` : `${count}, about ${size}`;
}
