import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest';
import type { ProjectRow } from '../bindings/ProjectRow';
import type { Snapshot } from '../bindings/Snapshot';
import { fixturePatch, fixtureSnapshot } from '../test/fixtures';

vi.mock('../bridge/commands', () => ({
  bridge: {
    apply: vi.fn(),
    undo: vi.fn(),
    redo: vi.fn(),
    snapshot: vi.fn(),
    createProject: vi.fn(),
    openProject: vi.fn(),
    closeProject: vi.fn(),
    duplicateProject: vi.fn(),
  },
}));
vi.mock('../bridge/projects', () => ({ projects: { recent: vi.fn(), reveal: vi.fn() } }));
const { bridge } = await import('../bridge/commands');
const { projects } = await import('../bridge/projects');
const { useProject } = await import('../state/project');
const { useRecentProjects } = await import('../state/recentProjects');
const { useSaving } = await import('../state/saving');
const { useToasts } = await import('../state/toasts');
const { TopBar } = await import('./TopBar');
const { useExport } = await import('../state/export');

/** The fixture project with nothing on its timeline. */
function emptySnapshot(): Snapshot {
  const snapshot = fixtureSnapshot();
  return { ...snapshot, project: { ...snapshot.project, timeline: { main: [], tracks: [] } } };
}

const LIBRARY = '/Users/me/Movies/Klipolo';

/** A project in the library as the start screen's list has it; `open`: the one open now. */
const recentRow = (name: string, open = false): ProjectRow => ({
  path: `${LIBRARY}/${name}.klipolo`,
  name,
  openedMs: 0,
  durationUs: 0,
  clips: 1,
  thumb: null,
  open,
});

/** The fixture, as Rust opens the project `name`. */
function openedAs(name: string, opening: number): Snapshot {
  const snapshot = fixtureSnapshot();
  return { ...snapshot, project: { ...snapshot.project, id: `id ${name}`, name }, opening };
}

/** The project menu, opened from the project's name. */
async function openMenu(name: string) {
  await userEvent.click(screen.getByRole('button', { name }));
  return screen.findByRole('menu');
}

beforeEach(() => {
  useProject.setState({ snapshot: null, pending: 0, switching: false });
  useSaving.setState({ failing: null, saved: null });
  useToasts.setState({ toasts: [] });
  vi.mocked(bridge.createProject).mockReset();
  vi.mocked(bridge.openProject).mockReset();
  vi.mocked(bridge.closeProject).mockReset();
  vi.mocked(projects.reveal).mockReset();
  vi.mocked(projects.recent).mockReset().mockResolvedValue([]);
  useRecentProjects.setState({ rows: [], readFor: null, readAt: 0 });
});

describe('TopBar', () => {
  it('top_bar_on_the_start_screen_has_no_undo_redo_or_export', () => {
    render(<TopBar />);
    const bar = screen.getByRole('banner');
    // The app icon, the AI chip and Settings.
    expect(within(bar).getByRole('img', { name: 'Klipolo' })).toBeInTheDocument();
    expect(within(bar).getByRole('button', { name: 'Settings' })).toBeInTheDocument();
    expect(within(bar).queryByRole('button', { name: /Undo|Redo/ })).toBeNull();
    expect(within(bar).queryByRole('button', { name: 'Export' })).toBeNull();
    expect(within(bar).queryByText('Untitled project')).toBeNull();
    expect(within(bar).queryByText('Saved')).toBeNull();
    expect(bar.querySelector('.project, .topbar-divider')).toBeNull();
  });

  it('says why Export is off without a video, and opens the export dialog with one', async () => {
    useProject.setState({ snapshot: emptySnapshot() });
    const { rerender } = render(<TopBar />);
    const exportButton = () => screen.getByRole('button', { name: 'Export' });
    expect(exportButton()).toHaveAttribute('aria-disabled', 'true');
    expect(exportButton().title).toBe('Add a video first');
    useProject.setState({ snapshot: fixtureSnapshot() });
    rerender(<TopBar />);
    expect(exportButton()).not.toHaveAttribute('aria-disabled');
    expect(exportButton().title).toBe('Export (Command E)');
    await userEvent.click(exportButton());
    expect(useExport.getState().open).toBe(true);
  });

  it('names the change Undo takes back, and undoes it', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    vi.mocked(bridge.undo).mockResolvedValue({ ...fixturePatch(fixtureSnapshot()), baseRevision: 3 });
    render(<TopBar />);
    expect(screen.getByRole('button', { name: 'Redo' })).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: 'Undo add text' }));
    expect(bridge.undo).toHaveBeenCalledTimes(1);
  });

  it('says the project is saved, saving, or why it is not', () => {
    const snapshot = { ...fixtureSnapshot(), opening: 1 };
    useProject.setState({ snapshot });
    useSaving.setState({ saved: { opening: 1, revision: snapshot.history.revision } });
    const { rerender } = render(<TopBar />);
    expect(screen.getByText('Saved')).toBeInTheDocument();
    useProject.setState({ snapshot: { ...snapshot, history: { ...snapshot.history, revision: 4 } } });
    rerender(<TopBar />);
    expect(screen.getByText('Saving…')).toBeInTheDocument();
    useSaving.setState({ failing: "Your disk is full, so changes aren't being saved. Free up some space." });
    rerender(<TopBar />);
    expect(screen.getByText('Not saved').title).toMatch(/disk is full/);
  });

  it('the project name opens the project menu', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    render(<TopBar />);
    const name = screen.getByRole('button', { name: 'Billing tutorial' });
    expect(name).toHaveAttribute('aria-haspopup', 'menu');
    expect(name.title).toBe('Billing tutorial');
    const menu = await openMenu('Billing tutorial');
    expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent)).toEqual([
      'New project',
      'All projects',
      'Rename',
      'Duplicate',
      'Show in Finder',
    ]);
  });

  it('Duplicate makes a copy of the project and opens it', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    const copy = fixtureSnapshot();
    copy.project = { ...copy.project, id: 'copy', name: 'Billing tutorial copy' };
    vi.mocked(bridge.duplicateProject).mockResolvedValue({ ...copy, opening: 2 });
    render(<TopBar />);
    await userEvent.click(within(await openMenu('Billing tutorial')).getByRole('menuitem', { name: 'Duplicate' }));
    // The open project: Rust copies it as it stands, every edit in.
    expect(bridge.duplicateProject).toHaveBeenCalledWith();
    await waitFor(() => expect(useProject.getState().snapshot?.project.id).toBe('copy'));
    expect(screen.getByRole('button', { name: 'Billing tutorial copy' })).toBeInTheDocument();
  });

  it("Duplicate is off for a project from a newer Klipolo, which this one can't copy whole", async () => {
    useProject.setState({ snapshot: { ...fixtureSnapshot(), readOnly: 'model.readOnly' } });
    render(<TopBar />);
    const menu = await openMenu('Billing tutorial');
    expect(within(menu).getByRole('menuitem', { name: 'Duplicate' })).toHaveAttribute('aria-disabled', 'true');
  });

  it('Rename names the project in one undo step, its words trimmed; Escape keeps the name', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    const renamed = fixtureSnapshot();
    renamed.project.name = 'Launch video';
    renamed.history = { ...renamed.history, revision: 4, undoLabel: 'editor.rename.undo' };
    vi.mocked(bridge.apply).mockReset().mockResolvedValue({ ...fixturePatch(renamed), baseRevision: 3 });
    render(<TopBar />);
    await userEvent.click(within(await openMenu('Billing tutorial')).getByRole('menuitem', { name: 'Rename' }));
    const field = await screen.findByRole('textbox', { name: 'Project name' });
    await waitFor(() => expect(field).toHaveFocus());
    expect(field).toHaveValue('Billing tutorial');
    await userEvent.clear(field);
    await userEvent.type(field, '  Launch video {Enter}');
    await waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'editor.rename.undo',
      ops: [{ type: 'rename', name: 'Launch video' }],
    });
    // The new name shows, Undo takes it back, and the focus is back on the name.
    const name = await screen.findByRole('button', { name: 'Launch video' });
    expect(screen.getByRole('button', { name: 'Undo rename project' })).toBeInTheDocument();
    await waitFor(() => expect(name).toHaveFocus());

    // Escape, or a name left empty, keeps the name.
    await userEvent.click(within(await openMenu('Launch video')).getByRole('menuitem', { name: 'Rename' }));
    await userEvent.type(await screen.findByRole('textbox', { name: 'Project name' }), ' 2{Escape}');
    expect(await screen.findByRole('button', { name: 'Launch video' })).toBeInTheDocument();
    await userEvent.click(within(await openMenu('Launch video')).getByRole('menuitem', { name: 'Rename' }));
    await userEvent.clear(await screen.findByRole('textbox', { name: 'Project name' }));
    await userEvent.keyboard('{Enter}');
    expect(await screen.findByRole('button', { name: 'Launch video' })).toBeInTheDocument();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
  });

  it('a rename stays as typed while the window is in the background; a click elsewhere then renames', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    const renamed = fixtureSnapshot();
    renamed.project.name = 'Launch video';
    renamed.history = { ...renamed.history, revision: 4, undoLabel: 'editor.rename.undo' };
    vi.mocked(bridge.apply).mockReset().mockResolvedValue({ ...fixturePatch(renamed), baseRevision: 3 });
    // jsdom says the page has no focus during any blur; WebKit says so only while the window is in the background.
    const pageFocused = vi.spyOn(document, 'hasFocus').mockReturnValue(true);
    onTestFinished(() => pageFocused.mockRestore());
    render(<TopBar />);
    await userEvent.click(within(await openMenu('Billing tutorial')).getByRole('menuitem', { name: 'Rename' }));
    const field = await screen.findByRole('textbox', { name: 'Project name' });
    await waitFor(() => expect(field).toHaveFocus());
    await userEvent.clear(field);
    await userEvent.type(field, 'Launch vi');
    // ⌘Tab: the window goes to the background and the field blurs. Back, WebKit gives it the focus again.
    pageFocused.mockReturnValue(false);
    act(() => field.blur());
    pageFocused.mockReturnValue(true);
    act(() => field.focus());
    // The field is still there, and typing goes on where it was.
    expect(field).toBeInTheDocument();
    await userEvent.keyboard('deo');
    expect(field).toHaveValue('Launch video');
    expect(bridge.apply).not.toHaveBeenCalled();
    await userEvent.click(document.body);
    await waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'editor.rename.undo',
      ops: [{ type: 'rename', name: 'Launch video' }],
    });
    expect(await screen.findByRole('button', { name: 'Launch video' })).toBeInTheDocument();
  });

  it('project_menu_new_project_creates_and_opens_it', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    const made = emptySnapshot();
    made.project = { ...made.project, id: 'new', name: 'Untitled project' };
    vi.mocked(bridge.createProject).mockResolvedValue({ ...made, opening: 2 });
    render(<TopBar />);
    await userEvent.click(within(await openMenu('Billing tutorial')).getByRole('menuitem', { name: 'New project' }));
    expect(bridge.createProject).toHaveBeenCalledWith('Untitled project', ['landscape', 'portrait', 'square']);
    await waitFor(() => expect(useProject.getState().snapshot?.project.id).toBe('new'));
    expect(screen.getByRole('button', { name: 'Untitled project' })).toBeInTheDocument();
    expect(bridge.closeProject).not.toHaveBeenCalled();
  });

  it('the project menu runs one switch at a time', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    const made = emptySnapshot();
    made.project = { ...made.project, id: 'new', name: 'Untitled project' };
    let land!: (snapshot: Snapshot) => void;
    vi.mocked(bridge.createProject).mockImplementation(() => new Promise((resolve) => (land = resolve)));
    render(<TopBar />);
    await userEvent.click(within(await openMenu('Billing tutorial')).getByRole('menuitem', { name: 'New project' }));
    expect(bridge.createProject).toHaveBeenCalledTimes(1);

    // While the new project is made, the menu's items are off: another New project or All projects would overlap it.
    const menu = await openMenu('Billing tutorial');
    const items = within(menu).getAllByRole('menuitem');
    expect(items.map((item) => item.getAttribute('aria-disabled'))).toEqual(Array(5).fill('true'));
    await userEvent.click(within(menu).getByRole('menuitem', { name: 'New project' }));
    await userEvent.click(within(menu).getByRole('menuitem', { name: 'All projects' }));
    expect(bridge.createProject).toHaveBeenCalledTimes(1);
    expect(bridge.closeProject).not.toHaveBeenCalled();
    await userEvent.keyboard('{Escape}');

    land({ ...made, opening: 2 });
    await waitFor(() => expect(useProject.getState().snapshot?.project.id).toBe('new'));
    // Once it has opened, the menu works again.
    const after = await openMenu('Untitled project');
    expect(within(after).getAllByRole('menuitem').map((item) => item.getAttribute('aria-disabled'))).toEqual([
      null,
      null,
      null,
      null,
      null,
    ]);
  });

  it('all projects closes the project', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    vi.mocked(bridge.closeProject).mockResolvedValue(undefined);
    render(<TopBar />);
    await userEvent.click(within(await openMenu('Billing tutorial')).getByRole('menuitem', { name: 'All projects' }));
    await waitFor(() => expect(useProject.getState().snapshot).toBeNull());
    expect(bridge.closeProject).toHaveBeenCalledTimes(1);
    // The bar is the start screen's now.
    expect(screen.queryByRole('button', { name: 'Billing tutorial' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Export' })).toBeNull();
  });

  it('show in finder shows the project file, or says why it cannot', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    vi.mocked(projects.reveal).mockResolvedValueOnce(undefined);
    render(<TopBar />);
    await userEvent.click(within(await openMenu('Billing tutorial')).getByRole('menuitem', { name: 'Show in Finder' }));
    expect(projects.reveal).toHaveBeenCalledTimes(1);
    expect(useToasts.getState().toasts).toEqual([]);

    vi.mocked(projects.reveal).mockRejectedValueOnce(new Error("This project's file isn't there any more."));
    await userEvent.click(within(await openMenu('Billing tutorial')).getByRole('menuitem', { name: 'Show in Finder' }));
    await waitFor(() =>
      expect(useToasts.getState().toasts.map((toast) => toast.text)).toEqual([
        "This project's file isn't there any more.",
      ]),
    );
  });

  it('open_recent_lists_the_recent_projects_without_the_open_one', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    vi.mocked(projects.recent).mockResolvedValue([
      recentRow('Kiln vlog'),
      recentRow('Billing tutorial', true),
      recentRow('Studio tour'),
      recentRow('Banner'),
      recentRow('Untitled project'),
      recentRow('Untitled project 2'),
      recentRow('Older'),
    ]);
    render(<TopBar />);
    const menu = await openMenu('Billing tutorial');
    // The five last opened, in the start screen's order, between All projects and Rename; the open one left out.
    await waitFor(() =>
      expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent)).toEqual([
        'New project',
        'All projects',
        'Kiln vlog',
        'Studio tour',
        'Banner',
        'Untitled project',
        'Untitled project 2',
        'Rename',
        'Duplicate',
        'Show in Finder',
      ]),
    );
    const group = within(menu).getByRole('group', { name: 'Open recent' });
    expect(within(group).getAllByRole('menuitem')).toHaveLength(5);
    // Each is named by its project's name: a long one is cut (the menu doesn't widen for it), whole in its tooltip.
    const kiln = within(group).getByRole('menuitem', { name: 'Kiln vlog' });
    expect(kiln.querySelector('.menu-item-text.is-name')).toHaveAttribute('title', 'Kiln vlog');
    const css = readFileSync(resolve(__dirname, '../styles/app.css'), 'utf8');
    expect(css).toMatch(/\.menu-item-text\.is-name \{\s*max-width: 280px;/);
    expect(menu.textContent).not.toMatch(/[–—]/);
  });

  it('open_recent_is_hidden_without_another_project', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    vi.mocked(projects.recent).mockResolvedValue([recentRow('Billing tutorial', true)]);
    render(<TopBar />);
    const menu = await openMenu('Billing tutorial');
    await waitFor(() => expect(useRecentProjects.getState().readFor).not.toBeNull());
    expect(within(menu).queryByRole('group')).toBeNull();
    expect(within(menu).queryByText('Open recent')).toBeNull();
    expect(within(menu).getAllByRole('menuitem')).toHaveLength(5);
    expect(menu.querySelectorAll('.menu-sep')).toHaveLength(1);
    await userEvent.keyboard('{Escape}');

    // A list that can't be read leaves it out too, quietly.
    useRecentProjects.setState({ rows: [], readFor: null, readAt: 0 });
    vi.mocked(projects.recent).mockRejectedValue(new Error("Klipolo couldn't read your projects folder."));
    const again = await openMenu('Billing tutorial');
    await waitFor(() => expect(vi.mocked(projects.recent).mock.calls.length).toBeGreaterThan(1));
    await act(() => Promise.resolve());
    expect(within(again).getAllByRole('menuitem')).toHaveLength(5);
    expect(useToasts.getState().toasts).toEqual([]);
  });

  it('picking_a_recent_project_opens_it', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    vi.mocked(projects.recent).mockResolvedValue([recentRow('Billing tutorial', true), recentRow('Kiln vlog')]);
    vi.mocked(bridge.openProject).mockResolvedValue(openedAs('Kiln vlog', 2));
    render(<TopBar />);
    const menu = await openMenu('Billing tutorial');
    await userEvent.click(await within(menu).findByRole('menuitem', { name: 'Kiln vlog' }));
    expect(bridge.openProject).toHaveBeenCalledWith(`${LIBRARY}/Kiln vlog.klipolo`);
    await waitFor(() => expect(useProject.getState().snapshot?.project.name).toBe('Kiln vlog'));
    expect(screen.getByRole('button', { name: 'Kiln vlog' })).toBeInTheDocument();

    // The list read for the project before isn't this one's: it is read again, and leaves this one out.
    vi.mocked(projects.recent).mockResolvedValue([recentRow('Kiln vlog', true), recentRow('Billing tutorial')]);
    const next = await openMenu('Kiln vlog');
    expect(await within(next).findByRole('menuitem', { name: 'Billing tutorial' })).toBeInTheDocument();
    expect(within(next).queryByRole('menuitem', { name: 'Kiln vlog' })).toBeNull();
  });

  it('a_recent_project_that_went_says_why_and_leaves_the_list', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    vi.mocked(projects.recent).mockResolvedValue([
      recentRow('Billing tutorial', true),
      recentRow('Kiln vlog'),
      recentRow('Banner'),
    ]);
    // The file went (moved or deleted) after the list was read.
    vi.mocked(bridge.openProject).mockImplementation(async () => {
      vi.mocked(projects.recent).mockResolvedValue([recentRow('Billing tutorial', true), recentRow('Banner')]);
      throw new Error("That project isn't there any more.");
    });
    render(<TopBar />);
    const menu = await openMenu('Billing tutorial');
    await userEvent.click(await within(menu).findByRole('menuitem', { name: 'Kiln vlog' }));
    // It says why, as a start screen card does; the project open stays open.
    await waitFor(() =>
      expect(useToasts.getState().toasts.map((toast) => toast.text)).toEqual([
        "That project isn't there any more.",
      ]),
    );
    expect(useProject.getState().snapshot?.project.name).toBe('Billing tutorial');
    // The list is read again: the one that went is gone from it.
    await waitFor(() => expect(useRecentProjects.getState().rows.map((row) => row.name)).toEqual(['Banner']));
    const again = await openMenu('Billing tutorial');
    expect(within(again).getByRole('menuitem', { name: 'Banner' })).toBeInTheDocument();
    expect(within(again).queryByRole('menuitem', { name: 'Kiln vlog' })).toBeNull();
  });

  it('the_arrow_keys_and_return_work_through_open_recent', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    vi.mocked(projects.recent).mockResolvedValue([
      recentRow('Billing tutorial', true),
      recentRow('Kiln vlog'),
      recentRow('Banner'),
    ]);
    vi.mocked(bridge.openProject).mockResolvedValue(openedAs('Banner', 2));
    render(<TopBar />);
    // The focus reaching the name reads the list, so it is there as the menu opens.
    act(() => screen.getByRole('button', { name: 'Billing tutorial' }).focus());
    await waitFor(() => expect(useRecentProjects.getState().rows).toHaveLength(2));
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(screen.getByRole('menuitem', { name: 'New project' })).toHaveFocus());
    await userEvent.keyboard('{ArrowDown}{ArrowDown}');
    expect(screen.getByRole('menuitem', { name: 'Kiln vlog' })).toHaveFocus();
    await userEvent.keyboard('{ArrowDown}');
    expect(screen.getByRole('menuitem', { name: 'Banner' })).toHaveFocus();
    await userEvent.keyboard('{ArrowDown}');
    expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveFocus();
    await userEvent.keyboard('{ArrowUp}');
    expect(screen.getByRole('menuitem', { name: 'Banner' })).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    expect(bridge.openProject).toHaveBeenCalledWith(`${LIBRARY}/Banner.klipolo`);
    await waitFor(() => expect(useProject.getState().snapshot?.project.name).toBe('Banner'));
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('open_recent_waits_while_another_switch_is_under_way', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    vi.mocked(projects.recent).mockResolvedValue([recentRow('Billing tutorial', true), recentRow('Kiln vlog')]);
    let land!: (snapshot: Snapshot) => void;
    vi.mocked(bridge.createProject).mockImplementation(() => new Promise((resolve) => (land = resolve)));
    render(<TopBar />);
    await userEvent.click(within(await openMenu('Billing tutorial')).getByRole('menuitem', { name: 'New project' }));
    const menu = await openMenu('Billing tutorial');
    const kiln = await within(menu).findByRole('menuitem', { name: 'Kiln vlog' });
    expect(kiln).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(kiln);
    expect(bridge.openProject).not.toHaveBeenCalled();
    await userEvent.keyboard('{Escape}');
    land(openedAs('Untitled project', 2));
    await waitFor(() => expect(useProject.getState().snapshot?.project.name).toBe('Untitled project'));
  });

  it('hovering_the_name_again_reuses_a_read_from_the_last_few_seconds', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    vi.mocked(projects.recent).mockResolvedValue([recentRow('Billing tutorial', true), recentRow('Kiln vlog')]);
    const now = vi.spyOn(Date, 'now').mockReturnValue(1_000_000);
    onTestFinished(() => now.mockRestore());
    render(<TopBar />);
    const name = screen.getByRole('button', { name: 'Billing tutorial' });
    await userEvent.hover(name);
    await waitFor(() => expect(useRecentProjects.getState().readFor).not.toBeNull());
    await userEvent.unhover(name);
    // A few seconds later: the same list, nothing read again.
    now.mockReturnValue(1_008_000);
    await userEvent.hover(name);
    await userEvent.unhover(name);
    expect(projects.recent).toHaveBeenCalledTimes(1);
    // Later than that: read again.
    now.mockReturnValue(1_011_000);
    await userEvent.hover(name);
    await waitFor(() => expect(projects.recent).toHaveBeenCalledTimes(2));
  });

  it('the_menu_waits_briefly_for_the_list_so_open_recent_is_there_as_it_opens', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    let answer!: (rows: ProjectRow[]) => void;
    vi.mocked(projects.recent).mockImplementation(() => new Promise((resolve) => (answer = resolve)));
    render(<TopBar />);
    await userEvent.click(screen.getByRole('button', { name: 'Billing tutorial' }));
    // The list for this project isn't read yet: the menu waits for it, a moment at most.
    expect(screen.queryByRole('menu')).toBeNull();
    act(() => answer([recentRow('Billing tutorial', true), recentRow('Kiln vlog')]));
    const menu = await screen.findByRole('menu');
    expect(within(menu).getByRole('group', { name: 'Open recent' })).toBeInTheDocument();
  });

  it('a_slow_list_never_moves_the_items_while_the_menu_is_open', async () => {
    useProject.setState({ snapshot: fixtureSnapshot() });
    let answer!: (rows: ProjectRow[]) => void;
    vi.mocked(projects.recent).mockImplementation(() => new Promise((resolve) => (answer = resolve)));
    render(<TopBar />);
    await userEvent.click(screen.getByRole('button', { name: 'Billing tutorial' }));
    // A read that takes longer than that: the menu opens without the group.
    const menu = await screen.findByRole('menu');
    expect(within(menu).queryByRole('group')).toBeNull();
    const items = () => within(menu).getAllByRole('menuitem').map((item) => item.textContent);
    const before = items();
    // It lands while the menu is open: nothing moves under the pointer.
    await act(async () => answer([recentRow('Billing tutorial', true), recentRow('Kiln vlog')]));
    await waitFor(() => expect(useRecentProjects.getState().rows).toHaveLength(1));
    expect(items()).toEqual(before);
    expect(within(menu).queryByRole('group')).toBeNull();
    // The next time it opens, the group is there.
    await userEvent.keyboard('{Escape}');
    const again = await openMenu('Billing tutorial');
    expect(within(again).getByRole('group', { name: 'Open recent' })).toBeInTheDocument();
  });
});
