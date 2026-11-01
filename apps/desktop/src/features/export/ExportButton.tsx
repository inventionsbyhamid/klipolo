import { useEffect } from 'react';
import { useIntlLocale } from '../../i18n/react';
import { Icon } from '../../layout/Icon';
import { formatPercent } from '../../lib/format';
import { videoDurationUs } from '../../lib/timeline';
import { exportPercent, unfinishedJobs, useExport } from '../../state/export';
import { useProject } from '../../state/project';

/** "Export, 2 unfinished exports": the button's name while exports can be resumed. */
function unfinishedName(count: number): string {
  return count === 1 ? 'Export, 1 unfinished export' : `Export, ${count} unfinished exports`;
}

/**
 * The top bar's Export (Command E): opens the export dialog. Off, saying why, until there is a video (a project of
 * layers alone is one) or an export to resume. While an export runs with the dialog closed it shows the progress, and
 * opens the dialog again; paused, it reads "Paused" (calm, not a failure). A dot marks unfinished exports that can
 * resume: never a dialog at launch.
 */
export function ExportButton() {
  const locale = useIntlLocale();
  const hasVideo = useProject((s) => (s.snapshot ? videoDurationUs(s.snapshot.project.timeline) > 0 : false));
  const running = useExport((s) => s.phase === 'running');
  const paused = useExport((s) => s.phase === 'paused');
  const finishing = useExport((s) => s.finishing);
  const percent = useExport(exportPercent);
  const unfinished = useExport((s) => unfinishedJobs(s).length);

  useEffect(() => {
    // The editor opened: the unfinished exports are listed once (the dialog lists them again when it opens).
    void useExport.getState().loadJobs();
  }, []);

  const text = running
    ? finishing
      ? 'Finishing'
      : `Exporting ${formatPercent(locale, percent / 100)}`
    : paused
      ? 'Paused'
      : 'Export';
  const dot = !running && !paused && unfinished > 0;
  const off = !hasVideo && !running && !paused && unfinished === 0;
  return (
    <button
      type="button"
      className={paused ? 'btn btn-secondary export-btn' : 'btn btn-primary export-btn'}
      // Off but not `disabled`: WebKit shows no tooltip on a disabled button, and this one says why.
      aria-disabled={off || undefined}
      aria-label={paused ? 'Export paused' : dot ? unfinishedName(unfinished) : undefined}
      title={off ? 'Add a video first' : 'Export (Command E)'}
      onClick={() => !off && useExport.getState().setOpen(true)}
    >
      <Icon name="export" />
      <span className="ex-tabular">{text}</span>
      {dot && <span className="export-dot" aria-hidden />}
    </button>
  );
}
