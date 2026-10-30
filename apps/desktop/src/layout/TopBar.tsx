import icon from '../styles/klipolo-icon.svg';
import { historyAction } from '../lib/history';
import { useProject } from '../state/project';
import { useSaving } from '../state/saving';
import { Icon } from './Icon';

/** Why Export is off while a video is open (export lands in a later build). */
export const EXPORT_LATER = 'Export comes in the next build.';

/** The top bar: the window's drag region, the project's name and whether it is saved, undo and redo, and Export. */
export function TopBar() {
  const name = useProject((s) => s.snapshot?.project.name ?? 'Untitled project');
  const open = useProject((s) => s.snapshot !== null);
  const hasVideo = useProject((s) => (s.snapshot?.project.timeline.main.length ?? 0) > 0);
  const history = useProject((s) => s.snapshot?.history);
  const failing = useSaving((s) => s.failing);
  const undo = historyAction('undo', history?.undoLabel ?? null, history?.undoParams);
  const redo = historyAction('redo', history?.redoLabel ?? null, history?.redoParams);
  return (
    <header className="topbar" data-tauri-drag-region>
      <img className="app-icon" src={icon} alt="Klipolo" width={26} height={26} />
      <div className="project" title={name}>
        <span className="project-name">{name}</span>
        <Icon name="chev-down" />
      </div>
      {open && (
        <span className="save-state" title={failing ?? undefined}>
          {failing ? 'Not saved' : 'Saved'}
        </span>
      )}
      <span className="topbar-divider" aria-hidden />
      <button
        className="icon-btn"
        aria-label={undo}
        title={`${undo} (Command Z)`}
        disabled={!history?.canUndo}
        onClick={() => void useProject.getState().undo()}
      >
        <Icon name="undo" />
      </button>
      <button
        className="icon-btn"
        aria-label={redo}
        title={`${redo} (Shift Command Z)`}
        disabled={!history?.canRedo}
        onClick={() => void useProject.getState().redo()}
      >
        <Icon name="redo" />
      </button>
      <div className="topbar-right" data-tauri-drag-region>
        <button className="icon-btn" aria-label="Settings" title="Settings (Command ,)" disabled>
          <Icon name="settings" />
        </button>
        <button className="btn btn-primary export-btn" disabled title={hasVideo ? EXPORT_LATER : 'Add a video first'}>
          <Icon name="export" />
          Export
        </button>
      </div>
    </header>
  );
}
