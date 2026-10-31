import { useLingui } from '@lingui/react/macro';
import { useEffect, useRef, useState } from 'react';
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
 * The project's name as a field, from the project menu's Rename: Return, or leaving the field, renames the project in
 * one undo step ("Rename project"; its exports take the new name); Escape, or a name left empty or as it was, keeps the
 * name. Return and Escape put the focus back on the name. The window going to the background leaves it as typed.
 */
function RenameField({ name, onDone }: { name: string; onDone: (refocus: boolean) => void }) {
  const { t } = useLingui();
  const [value, setValue] = useState(name);
  const finished = useRef(false);
  /** Its first focus selects the name; the focus the window gives back keeps what was typed, and where. */
  const selected = useRef(false);
  const finish = (rename: boolean, refocus: boolean) => {
    if (finished.current) return;
    finished.current = true;
    const next = value.trim();
    if (rename && next !== '' && next !== name)
      void useProject.getState().apply('editor.rename.undo', [{ type: 'rename', name: next }]);
    onDone(refocus);
  };
  return (
    <input
      className="input input-sm project-rename"
      aria-label={t({ id: 'project.rename.field', message: 'Project name' })}
      value={value}
      // The model's limit for a project's name.
      maxLength={200}
      autoFocus
      onFocus={(e) => {
        if (selected.current) return;
        selected.current = true;
        e.currentTarget.select();
      }}
      onChange={(e) => setValue(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          finish(true, true);
        } else if (e.key === 'Escape') {
          e.preventDefault();
          e.stopPropagation();
          finish(false, true);
        }
      }}
      // Not when the window goes to the background (⌘Tab): the field stays as typed and gets the focus back with it.
      onBlur={() => document.hasFocus() && finish(true, false)}
    />
  );
}

/**
 * The project menu, opened from the project's name: New project, All projects (closes the project once
 * its edits are saved, back to the start screen), Rename (the name turns into a field) and Show in Finder (its file,
 * selected). One at a time: while a new project is made or the project closes, its items are off.
 */
function ProjectMenu({ name }: { name: string }) {
  const { t } = useLingui();
  const switching = useProject((s) => s.switching);
  const readOnly = useProject((s) => s.snapshot?.readOnly != null);
  const [renaming, setRenaming] = useState(false);
  const nameButton = useRef<HTMLButtonElement>(null);
  /** The rename ended from the keyboard: the focus goes back to the name once it shows again. */
  const refocus = useRef(false);
  useEffect(() => {
    if (renaming || !refocus.current) return;
    refocus.current = false;
    nameButton.current?.focus();
  }, [renaming]);
  /** Runs `action` unless a switch is under way (a press that reached the item before it turned off). */
  const once = (action: () => void) => () => {
    if (!useProject.getState().switching) action();
  };
  const showInFinder = () => void projects.reveal().catch((error: unknown) => toast.error(errorMessage(error)));
  if (renaming)
    return (
      <RenameField
        name={name}
        onDone={(back) => {
          refocus.current = back;
          setRenaming(false);
        }}
      />
    );
  return (
    <Menu>
      <MenuTrigger asChild>
        <button ref={nameButton} type="button" className="project" title={name}>
          <span className="project-name">{name}</span>
          <Icon name="chev-down" />
        </button>
      </MenuTrigger>
      <MenuContent>
        <MenuItem onSelect={once(() => void newProject())} disabled={switching}>
          {t({ id: 'project.menu.new', message: 'New project', comment: 'Project menu item' })}
        </MenuItem>
        <MenuItem onSelect={once(() => void useProject.getState().close())} disabled={switching}>
          {t({
            id: 'project.menu.all',
            message: 'All projects',
            comment: 'Project menu item: closes the project and goes back to the start screen',
          })}
        </MenuItem>
        <MenuSeparator />
        <MenuItem onSelect={once(() => setRenaming(true))} disabled={switching || readOnly}>
          {t({ id: 'project.menu.rename', message: 'Rename', comment: 'Project menu item: the name becomes a field' })}
        </MenuItem>
        <MenuItem onSelect={once(showInFinder)} disabled={switching}>
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
