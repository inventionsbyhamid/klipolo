import icon from '../styles/klipolo-icon.svg';
import { useSession } from '../player/session';
import { Icon } from './Icon';

/** Why Export is off while a video is open (export lands in a later build). */
export const EXPORT_LATER = 'Export comes in the next build.';

/** The top bar: the window's drag region, the project's name, and placeholders for undo, settings and export. */
export function TopBar() {
  const name = useSession((s) => s.project?.name ?? 'Untitled project');
  const hasProject = useSession((s) => !!s.project);
  return (
    <header className="topbar" data-tauri-drag-region>
      <img className="app-icon" src={icon} alt="Klipolo" width={26} height={26} />
      <div className="project" title={name}>
        <span className="project-name">{name}</span>
        <Icon name="chev-down" />
      </div>
      <span className="topbar-divider" aria-hidden />
      <button className="icon-btn" aria-label="Undo" title="Undo (Command Z)" disabled>
        <Icon name="undo" />
      </button>
      <button className="icon-btn" aria-label="Redo" title="Redo (Shift Command Z)" disabled>
        <Icon name="redo" />
      </button>
      <div className="topbar-right" data-tauri-drag-region>
        <button className="icon-btn" aria-label="Settings" title="Settings (Command ,)" disabled>
          <Icon name="settings" />
        </button>
        <button className="btn btn-primary export-btn" disabled title={hasProject ? EXPORT_LATER : 'Add a video first'}>
          <Icon name="export" />
          Export
        </button>
      </div>
    </header>
  );
}
