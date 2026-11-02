import { Button } from '../../design/Button';
import { Icon } from '../../layout/Icon';
import { captionFailure, exportedTitle } from '../../state/exportRun';
import { revealExported, shareExported, useExport } from '../../state/export';
import { ExportFiles } from './ExportFiles';
import { ExportIcon } from './icons';

/**
 * The finished export: a paper check, "Exported 2 videos", where they were saved, one row per
 * file (a captions file saved beside a video too, or why it wasn't), then Done, Share… (the Mac's share menu with every
 * file, beside the button) and Show in Finder (their folder, the first file selected).
 */
export function ExportOutcome({ onClose }: { onClose: () => void }) {
  const count = useExport((s) => s.ready.length);
  const folder = useExport((s) => s.folder);
  const captionFailed = useExport((s) => s.captionFailed);
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
        <button
          type="button"
          // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
          tabIndex={0}
          className="icon-btn"
          aria-label="Close"
          title="Close (Esc)"
          onClick={onClose}
        >
          <ExportIcon name="x" />
        </button>
      </div>
      <ExportFiles />
      {/* The video is in place; its captions file isn't. */}
      {captionFailed && (
        <p className="ex-note">
          <Icon name="info" />
          <span>{captionFailure(captionFailed)}</span>
        </p>
      )}
      <div className="dialog-foot">
        <Button data-primary onClick={onClose}>
          Done
        </Button>
        <Button onClick={(e) => void shareExported(e.currentTarget)}>Share…</Button>
        <Button variant="primary" onClick={() => void revealExported()}>
          <ExportIcon name="folder" />
          Show in Finder
        </Button>
      </div>
    </>
  );
}
