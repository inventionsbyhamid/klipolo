import icon from '../styles/klipolo-icon.svg';
import { openSettings } from '../ai/aiStatus';
import { ExportButton } from '../features/export/ExportButton';
import { historyAction } from '../lib/history';
import { AiChip } from '../settings/AiChip';
import { useProject } from '../state/project';
import { saveStateOf, useSaving } from '../state/saving';
import { Icon } from './Icon';

/** The top bar: the window's drag region, the project's name and whether it is saved, undo and redo, and Export. */
export function TopBar() {
  const name = useProject((s) => s.snapshot?.project.name ?? 'Untitled project');
  const open = useProject((s) => s.snapshot !== null);
  const history = useProject((s) => s.snapshot?.history);
  const snapshot = useProject((s) => s.snapshot);
  const saving = useSaving();
  const state = saveStateOf(snapshot, saving);
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
        <span className="save-state" title={saving.failing ?? undefined}>
          {state === 'failing' ? 'Not saved' : state === 'saving' ? 'Saving…' : 'Saved'}
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
        <AiChip />
        <button className="icon-btn" aria-label="Settings" title="Settings (Command ,)" onClick={openSettings}>
          <Icon name="settings" />
        </button>
        <ExportButton />
      </div>
    </header>
  );
}
