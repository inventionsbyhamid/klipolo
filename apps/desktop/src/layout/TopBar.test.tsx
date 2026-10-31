import { render, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Snapshot } from '../bindings/Snapshot';
import { fixturePatch, fixtureSnapshot } from '../test/fixtures';

vi.mock('../bridge/commands', () => ({
  bridge: {
    apply: vi.fn(),
    undo: vi.fn(),
    redo: vi.fn(),
    snapshot: vi.fn(),
    createProject: vi.fn(),
    closeProject: vi.fn(),
  },
}));
vi.mock('../bridge/projects', () => ({ projects: { recent: vi.fn(), reveal: vi.fn() } }));
const { bridge } = await import('../bridge/commands');
const { projects } = await import('../bridge/projects');
const { useProject } = await import('../state/project');
const { useSaving } = await import('../state/saving');
const { useToasts } = await import('../state/toasts');
const { TopBar } = await import('./TopBar');
const { useExport } = await import('../state/export');

/** The fixture project with nothing on its timeline. */
function emptySnapshot(): Snapshot {
  const snapshot = fixtureSnapshot();
  return { ...snapshot, project: { ...snapshot.project, timeline: { main: [], tracks: [] } } };
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
  vi.mocked(bridge.closeProject).mockReset();
  vi.mocked(projects.reveal).mockReset();
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
    expect(exportButton()).toBeDisabled();
    expect(exportButton().title).toBe('Add a video first');
    useProject.setState({ snapshot: fixtureSnapshot() });
    rerender(<TopBar />);
    expect(exportButton()).toBeEnabled();
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
      'Show in Finder',
    ]);
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
    expect(items.map((item) => item.getAttribute('aria-disabled'))).toEqual(['true', 'true', 'true']);
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
});
