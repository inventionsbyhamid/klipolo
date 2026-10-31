import { exporter } from '../../bridge/exporter';
import { Button } from '../../design/Button';
import { Icon } from '../../layout/Icon';
import { exportedTitle } from '../../state/exportRun';
import { useExport } from '../../state/export';
import { ExportFiles } from './ExportFiles';
import { ExportIcon } from './icons';

/**
 * The finished export: a paper check, "Exported 2 videos", where they were saved, one row per
 * file, then Done and Show in Finder (every file, selected together).
 */
export function ExportOutcome({ onClose }: { onClose: () => void }) {
  const count = useExport((s) => s.ready.length);
  const folder = useExport((s) => s.folder);
  return (
    <>
      <div className="dialog-head">
        <span className="ex-done-mark" aria-hidden>
          <Icon name="check" />
        </span>
        <div>
          <h2 className="dialog-title" id="export-title" tabIndex={-1}>
            {exportedTitle(count)}
          </h2>
          {folder && <p className="dialog-sub">Saved in {folder}.</p>}
        </div>
        <button type="button" className="icon-btn" aria-label="Close" title="Close (Esc)" onClick={onClose}>
          <ExportIcon name="x" />
        </button>
      </div>
      <ExportFiles />
      <div className="dialog-foot">
        <Button data-primary onClick={onClose}>
          Done
        </Button>
        <Button variant="primary" onClick={() => void exporter.reveal().catch(() => undefined)}>
          <ExportIcon name="folder" />
          Show in Finder
        </Button>
      </div>
    </>
  );
}
