import { useLingui } from '@lingui/react/macro';
import icon from '../styles/klipolo-icon.svg';
import { openSettings } from '../ai/aiStatus';
import { projects } from '../bridge/projects';
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from '../design/Menu';
import { ExportButton } from '../features/export/ExportButton';
import { errorMessage } from '../lib/errors';
import { historyAction } from '../lib/history';
import { AiChip } from '../settings/AiChip';
import { newProject, useProject } from '../state/project';
import { saveStateOf, useSaving } from '../state/saving';
import { toast } from '../state/toasts';
import { Icon } from './Icon';

/**
 * The project menu, opened from the project's name: New project, All projects (closes the project once
 * its edits are saved, back to the start screen) and Show in Finder (its file, selected).
 */
function ProjectMenu({ name }: { name: string }) {
  const { t } = useLingui();
  const showInFinder = () => void projects.reveal().catch((error: unknown) => toast.error(errorMessage(error)));
  return (
    <Menu>
      <MenuTrigger asChild>
        <button type="button" className="project" title={name}>
          <span className="project-name">{name}</span>
          <Icon name="chev-down" />
        </button>
      </MenuTrigger>
      <MenuContent>
        <MenuItem onSelect={() => void newProject()}>
          {t({ id: 'project.menu.new', message: 'New project', comment: 'Project menu item' })}
        </MenuItem>
        <MenuItem onSelect={() => void useProject.getState().close()}>
          {t({
            id: 'project.menu.all',
            message: 'All projects',
            comment: 'Project menu item: closes the project and goes back to the start screen',
          })}
        </MenuItem>
        <MenuSeparator />
        <MenuItem onSelect={showInFinder}>
          {t({ id: 'project.menu.finder', message: 'Show in Finder', comment: 'Project menu item' })}
        </MenuItem>
      </MenuContent>
    </Menu>
  );
}

/**
 * The top bar: the window's drag region. With a project open: its name (the project menu), whether it is saved, undo
 * and redo, and Export. On the start screen only the app icon, the AI chip and Settings.
 */
export function TopBar() {
  const snapshot = useProject((s) => s.snapshot);
  const saving = useSaving();
  const state = saveStateOf(snapshot, saving);
  const history = snapshot?.history;
  const undo = historyAction('undo', history?.undoLabel ?? null, history?.undoParams);
  const redo = historyAction('redo', history?.redoLabel ?? null, history?.redoParams);
  return (
    <header className="topbar" data-tauri-drag-region>
      <img className="app-icon" src={icon} alt="Klipolo" width={26} height={26} />
      {snapshot && (
        <>
          <ProjectMenu name={snapshot.project.name} />
          <span className="save-state" title={saving.failing ?? undefined}>
            {state === 'failing' ? 'Not saved' : state === 'saving' ? 'Saving…' : 'Saved'}
          </span>
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
        </>
      )}
      <div className="topbar-right" data-tauri-drag-region>
        <AiChip />
        <button className="icon-btn" aria-label="Settings" title="Settings (Command ,)" onClick={openSettings}>
          <Icon name="settings" />
        </button>
        {snapshot && <ExportButton />}
      </div>
    </header>
  );
}
