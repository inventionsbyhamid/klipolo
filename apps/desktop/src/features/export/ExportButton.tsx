import { useIntlLocale } from '../../i18n/react';
import { Icon } from '../../layout/Icon';
import { formatPercent } from '../../lib/format';
import { videoDurationUs } from '../../lib/timeline';
import { exportPercent, useExport } from '../../state/export';
import { useProject } from '../../state/project';

/**
 * The top bar's Export (Command E): opens the export dialog. Off, saying why, until there is a video (a project of
 * layers alone is one). While an export runs with the dialog closed it shows the progress, and opens the dialog again.
 */
export function ExportButton() {
  const locale = useIntlLocale();
  const hasVideo = useProject((s) => (s.snapshot ? videoDurationUs(s.snapshot.project.timeline) > 0 : false));
  const running = useExport((s) => s.phase === 'running');
  const finishing = useExport((s) => s.finishing);
  const percent = useExport(exportPercent);
  const text = !running
    ? 'Export'
    : finishing
      ? 'Finishing'
      : `Exporting ${formatPercent(locale, percent / 100)}`;
  const off = !hasVideo && !running;
  return (
    <button
      type="button"
      className="btn btn-primary export-btn"
      // Off but not `disabled`: WebKit shows no tooltip on a disabled button, and this one says why.
      aria-disabled={off || undefined}
      title={off ? 'Add a video first' : 'Export (Command E)'}
      onClick={() => !off && useExport.getState().setOpen(true)}
    >
      <Icon name="export" />
      <span className="ex-tabular">{text}</span>
    </button>
  );
}
