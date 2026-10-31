import { type KeyboardEvent, useEffect, useRef, useState } from 'react';
import { useShallow } from 'zustand/react/shallow';
import type { ExportPlan } from '../../bindings/ExportPlan';
import type { FormatKind } from '../../bindings/FormatKind';
import { exporter } from '../../bridge/exporter';
import { Button } from '../../design/Button';
import { useIntlLocale } from '../../i18n/react';
import { Icon } from '../../layout/Icon';
import { formatFileSize } from '../../lib/format';
import { FORMAT_SHORT } from '../../lib/layout';
import { videoDurationUs } from '../../lib/timeline';
import { formatTime } from '../../player/time';
import { useExport } from '../../state/export';
import { notSaved } from '../../state/exportRun';
import { useProject } from '../../state/project';
import { toast } from '../../state/toasts';
import { ExportFiles } from './ExportFiles';
import { ExportOutcome } from './ExportOutcome';
import { ExportProgress } from './ExportProgress';
import { FormatCards } from './FormatCards';
import { ExportIcon } from './icons';
import { FORMATS, videos } from './presets';
import { QualityPicker } from './QualityPicker';
import './export.css';

const FOCUSABLE = 'button:not(:disabled), [tabindex="0"]';

type View = 'choose' | 'running' | 'failed' | 'done';

/**
 * The export dialog: pick the formats and the quality and where the files go, then
 * export; while it runs, one bar and Cancel export; when it is done, the files with Open and Show in Finder; when it
 * fails, what happened and Try again. Mounted once; opens from the top bar's Export or Command E. Esc and the close
 * button close it, and focus goes back where it was; an export still running carries on behind it (the top bar shows
 * its progress), as only Cancel export stops one.
 */
export function ExportDialog() {
  const open = useExport((s) => s.open);
  const project = useProject((s) => s.snapshot !== null);
  return open && project ? <Dialog /> : null;
}

function viewOf(phase: string): View {
  if (phase === 'running') return 'running';
  if (phase === 'done') return 'done';
  if (phase === 'failed') return 'failed';
  return 'choose';
}

function Dialog() {
  const view = useExport((s) => viewOf(s.phase));
  const announcement = useExport((s) => s.announcement);
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
        {/* The one status region: mounted with the dialog, so what it says next is announced. */}
        <p role="status" className="sr-only">
          {announcement}
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
  const checked: FormatKind[] =
    view === 'choose'
      ? FORMATS.filter((f) => (run.picked ?? [primaryFormat]).includes(f))
      : run.files.length > 0
        ? run.files.map((f) => f.format)
        : run.formats;
  const key = checked.join();

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
  }, [view, key, quality, folder]);

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
      <FormatCards
        checked={checked}
        files={view === 'choose' ? (plan?.files ?? []) : run.files}
        locked={view !== 'choose'}
        onToggle={toggle}
      />
      {view === 'choose' && <Options folder={plan?.folder ?? folder} />}
      {view === 'running' && <ExportProgress />}
      {view === 'failed' && <Failed />}
      {view === 'choose' && <ChooseFoot count={checked.length} bytes={plan?.bytes ?? null} formats={checked} onClose={onClose} locale={locale} />}
      {view === 'running' && <RunningFoot />}
      {view === 'failed' && <FailedFoot onClose={onClose} />}
    </>
  );
}

/** Quality and where the files go. */
function Options({ folder }: { folder: string | null }) {
  return (
    <dl className="ex-opts">
      <dt>Quality</dt>
      <dd>
        <QualityPicker />
      </dd>
      <dt>Save to</dt>
      <dd>
        <span className="ex-path">
          <ExportIcon name="folder" />
          {folder ?? 'Movies / Klipolo'}
        </span>
        <Button variant="ghost" size="sm" onClick={() => void useExport.getState().pickFolder()}>
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
  formats,
  onClose,
  locale,
}: {
  count: number;
  bytes: number | null;
  formats: FormatKind[];
  onClose: () => void;
  locale: string;
}) {
  const none = count === 0;
  const stopped = useExport((s) => s.phase === 'cancelled');
  const summary = none
    ? 'Pick at least one format'
    : bytes !== null && bytes > 0
      ? `${videos(count)}, about ${formatFileSize(locale, bytes)}`
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
        aria-describedby={none ? 'export-summary' : undefined}
        onClick={() => !none && void useExport.getState().start(formats)}
      >
        <Icon name="export" />
        {none ? 'Export' : `Export ${videos(count)}`}
      </Button>
    </div>
  );
}

/** The formats already in place, Cancel export, and the primary turned "Exporting" with a spinner. */
function RunningFoot() {
  const done = useExport(
    useShallow((s) => s.files.filter((_, i) => s.ready.includes(i)).map((f) => FORMAT_SHORT[f.format])),
  );
  const stopping = useExport((s) => s.stopping);
  const cancel = () =>
    void useExport
      .getState()
      .cancel()
      .then((refused) => {
        if (refused !== null) toast.error(refused);
      });
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
      <Button data-primary aria-disabled={stopping || undefined} onClick={() => !stopping && cancel()}>
        Cancel export
      </Button>
      <Button variant="primary" disabled>
        <span className="ex-spinner" aria-hidden />
        Exporting
      </Button>
    </div>
  );
}

/** What went wrong, and which formats weren't saved (failed); the files in place stay listed. */
function Failed() {
  const message = useExport((s) => s.message);
  const lost = useExport(
    useShallow((s) => s.files.filter((_, i) => !s.ready.includes(i)).map((f) => f.format)),
  );
  const said = [message, notSaved(lost)].filter(Boolean).join(' ');
  return (
    <>
      <div className="ex-progress">
        <p className="ex-failed">
          <Icon name="info" />
          <span>{said}</span>
        </p>
      </div>
      <ExportFiles />
    </>
  );
}

function FailedFoot({ onClose }: { onClose: () => void }) {
  return (
    <div className="dialog-foot">
      <Button onClick={onClose}>Close</Button>
      <Button variant="primary" data-primary onClick={() => void useExport.getState().retry()}>
        <Icon name="export" />
        Try again
      </Button>
    </div>
  );
}
