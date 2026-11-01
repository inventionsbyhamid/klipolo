import { useLingui } from '@lingui/react/macro';
import { useEffect, useRef, useState } from 'react';
import type { ProjectRow } from '../bindings/ProjectRow';
import icon from '../styles/klipolo-icon.svg';
import { openSettings } from '../ai/aiStatus';
import { projects } from '../bridge/projects';
import { Menu, MenuContent, MenuGroup, MenuItem, MenuSeparator, MenuTrigger } from '../design/Menu';
import { ExportButton } from '../features/export/ExportButton';
import { MediaJobsPill } from '../features/media-jobs/MediaJobsPill';
import { errorMessage } from '../lib/errors';
import { historyAction } from '../lib/history';
import { AiChip } from '../settings/AiChip';
import { newProject, shownName, useProject } from '../state/project';
import { openRecent, openRecentNow, readRecent, readRecentIfStale } from '../state/recentProjects';
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
 * How long the project menu waits for Open recent's list the first time it opens for a project: long enough for a read,
 * short enough that the menu still opens at once.
 */
const HOLD_MS = 200;

/**
 * The project menu, opened from the project's name: New project, All projects (closes the project once
 * its edits are saved, back to the start screen), Open recent (up to five other projects, last opened first: one opens
 * as its start screen card does), Rename (the name turns into a field), Duplicate (a copy with the same footage and
 * edits, opened as "<name> copy", to make another video from it) and Show in Finder (its file, selected). One at a time:
 * while a new project is made, another opens or the project closes, its items are off.
 */
function ProjectMenu({ name }: { name: string }) {
  const { t } = useLingui();
  const switching = useProject((s) => s.switching);
  const readOnly = useProject((s) => s.snapshot?.readOnly != null);
  const [open, setOpen] = useState(false);
  /** Open recent as the menu opened: it never changes while the menu is open, so nothing moves under the pointer. */
  const [recent, setRecent] = useState<ProjectRow[]>([]);
  /** The latest open asked for (a close, or an open that showed, moves it on). */
  const asked = useRef(0);
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
  /**
   * Opens the menu with Open recent as read for this project. The first time, its list isn't read yet: the menu waits
   * for it, `HOLD_MS` at most, so the group is there from the first paint (a slower read shows it the next time).
   */
  const openMenu = () => {
    const ask = ++asked.current;
    const show = () => {
      if (asked.current !== ask) return;
      asked.current++;
      setRecent(openRecentNow() ?? []);
      setOpen(true);
    };
    if (openRecentNow() !== null) {
      readRecentIfStale();
      show();
      return;
    }
    const timer = setTimeout(show, HOLD_MS);
    void readRecent().then(() => {
      clearTimeout(timer);
      show();
    });
  };
  const onOpenChange = (next: boolean) => {
    if (next) return openMenu();
    asked.current++;
    setOpen(false);
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
    // The list is read as the menu is about to open (the pointer or the focus on the name) and as it opens.
    <Menu open={open} onOpenChange={onOpenChange}>
      <MenuTrigger asChild onPointerEnter={readRecentIfStale} onFocus={readRecentIfStale}>
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
        {recent.length > 0 && (
          <>
            <MenuSeparator />
            <MenuGroup
              label={t({
                id: 'project.menu.recent',
                message: 'Open recent',
                comment: 'Project menu: the heading over the projects opened lately, each named by its name',
              })}
            >
              {recent.map((row) => (
                <MenuItem key={row.path} onSelect={once(() => void openRecent(row.path))} disabled={switching}>
                  <span className="menu-item-text is-name" title={row.name}>
                    {row.name}
                  </span>
                </MenuItem>
              ))}
            </MenuGroup>
          </>
        )}
        <MenuSeparator />
        <MenuItem onSelect={once(() => setRenaming(true))} disabled={switching || readOnly}>
          {t({ id: 'project.menu.rename', message: 'Rename', comment: 'Project menu item: the name becomes a field' })}
        </MenuItem>
        <MenuItem onSelect={once(() => void useProject.getState().duplicate())} disabled={switching || readOnly}>
          {t({
            id: 'project.menu.duplicate',
            message: 'Duplicate',
            comment: 'Project menu item: a copy of the project, opened as "<name> copy"',
          })}
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
 * and redo, the editing copies' pill while copies are made, and Export. On the start screen only the app icon, the AI
 * chip and Settings.
 */
export function TopBar() {
  const snapshot = useProject((s) => s.snapshot);
  /** Where the focus goes when the editing copies' pill goes while it had it. */
  const settings = useRef<HTMLButtonElement>(null);
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
          <ProjectMenu name={shownName(snapshot)} />
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
        {snapshot && <MediaJobsPill focusAfter={settings} />}
        <AiChip />
        <button
          ref={settings}
          className="icon-btn"
          aria-label="Settings"
          title="Settings (Command ,)"
          onClick={openSettings}
        >
          <Icon name="settings" />
        </button>
        {snapshot && <ExportButton />}
      </div>
    </header>
  );
}
