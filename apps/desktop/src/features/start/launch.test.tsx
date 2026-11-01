/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, render, renderHook, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ProjectRow } from '../../bindings/ProjectRow';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixtureSnapshot, revisionPatch } from '../../test/fixtures';

// The editor's panels stand in for themselves: these tests are about which screen the window shows, not what the
// editor draws.
vi.mock('../../layout/Library', () => ({ Library: () => <aside aria-label="Library" /> }));
vi.mock('../../preview/Preview', () => ({ Preview: () => <main aria-label="Preview" /> }));
vi.mock('../../features/timeline/Timeline', () => ({ Timeline: () => <section aria-label="Timeline" /> }));
vi.mock('../../layout/Chat', () => ({ Chat: () => <aside aria-label="Chat" /> }));
vi.mock('../../player/PlayerBar', () => ({ PlayerBar: () => <footer aria-label="Player" /> }));

const LIBRARY = '/Users/me/Movies/Klipolo';

const row = (name: string, openedMs: number): ProjectRow => ({
  path: `${LIBRARY}/${name}.klipolo`,
  name,
  openedMs,
  durationUs: 61_400_000,
  clips: 1,
  thumb: null,
  open: false,
});

/** A project as Rust opens it: the fixture, under its own id and name. */
function opened(name: string, opening: number): Snapshot {
  const snapshot = fixtureSnapshot();
  return { ...snapshot, project: { ...snapshot.project, id: `id ${name}`, name }, opening };
}

/**
 * Rust as launch, the start screen and the project menu see it: `open` is the project open now (none at launch),
 * `rows` the library's list. Every command is kept in `heard`.
 */
const rust = {
  open: null as Snapshot | null,
  rows: [] as ProjectRow[],
  heard: [] as string[],
  openings: 0,
};

function fakeRust() {
  mockIPC(
    async (cmd, args: any) => {
      rust.heard.push(cmd);
      switch (cmd) {
        case 'project_snapshot':
          if (!rust.open) throw { code: 'project.none', params: {} };
          return rust.open;
        case 'projects_recent':
          return rust.rows;
        case 'project_create':
          rust.open = opened(args.name, ++rust.openings);
          return rust.open;
        case 'project_open': {
          const found = rust.rows.find((r) => r.path === args.path);
          if (!found) throw "That project isn't there any more.";
          rust.open = opened(found.name, ++rust.openings);
          return rust.open;
        }
        case 'project_close':
          rust.open = null;
          return null;
        case 'project_apply':
          return revisionPatch(4);
        case 'engine_attach':
          return { port: 0, token: '', maxPreviewPixels: 0, adapter: '', presenter: 'socket', gpuClass: 'software' };
        case 'analysis_states':
          return [];
        default:
          return null;
      }
    },
    { shouldMockEvents: true },
  );
}

/**
 * Klipolo launching: a fresh page (the app's once-per-load start runs again) over `rust` as it is. Gives the page's
 * own project store, toolbar bursts and commit queue.
 */
async function launch() {
  vi.resetModules();
  fakeRust();
  const { App } = await import('../../App');
  const { useProject } = await import('../../state/project');
  const { useBurst } = await import('../../state/useBurst');
  const { beginCommit } = await import('../../state/commitQueue');
  render(<App />);
  return { useProject, useBurst, beginCommit };
}

const startScreen = () => screen.queryByRole('main', { name: 'Your projects' });
const editor = () => screen.queryByRole('main', { name: 'Preview' });
const card = (name: string) => within(startScreen()!).getByRole('button', { name });
const topBar = () => document.querySelector<HTMLElement>('header.topbar')!;

afterEach(() => {
  clearMocks();
  Object.assign(rust, { open: null, rows: [], heard: [], openings: 0 });
});

describe('launch', () => {
  it('launch_opens_no_project_and_shows_the_start_screen', async () => {
    rust.rows = [row('Studio tour', Date.now() - 60_000)];
    await launch();
    expect(await screen.findByRole('button', { name: 'Studio tour' })).toBeInTheDocument();
    // Rust was asked what is open (nothing), and nothing was made or opened.
    expect(rust.heard).toContain('project_snapshot');
    expect(rust.heard.filter((cmd) => cmd.startsWith('project_') && cmd !== 'project_snapshot')).toEqual([]);
    // The start screen alone: one main, no editor, no player bar.
    expect(screen.getAllByRole('main')).toEqual([startScreen()]);
    expect(editor()).toBeNull();
    expect(screen.queryByRole('contentinfo', { name: 'Player' })).toBeNull();
    // The top bar holds the app icon, the AI chip and Settings.
    expect(within(topBar()).getByRole('button', { name: 'Settings' })).toBeInTheDocument();
    expect(within(topBar()).queryByRole('button', { name: 'Export' })).toBeNull();
    // New project, and Record your screen beside it.
    expect(within(startScreen()!).getByRole('button', { name: 'New project' })).toBeInTheDocument();
    expect(within(startScreen()!).getByRole('button', { name: 'Record your screen' })).toBeInTheDocument();
    // Launch leaves the focus alone.
    expect(document.activeElement).toBe(document.body);
  });

  it('new_project_right_after_launch_makes_and_opens_it', async () => {
    await launch();
    await userEvent.click(await screen.findByRole('button', { name: 'New project' }));
    expect(await screen.findByRole('main', { name: 'Preview' })).toBeInTheDocument();
    expect(rust.heard.filter((cmd) => cmd === 'project_create')).toHaveLength(1);
    expect(within(topBar()).getByRole('button', { name: 'Untitled project' })).toBeInTheDocument();
  });

  it('a_project_that_cant_be_opened_says_why_on_the_start_screen', async () => {
    rust.rows = [row('Studio tour', Date.now() - 60_000), row('Kiln vlog', Date.now() - 120_000)];
    await launch();
    const kiln = await screen.findByRole('button', { name: 'Kiln vlog' });
    // The file went after the list was read.
    rust.rows = rust.rows.slice(0, 1);
    await userEvent.click(kiln);
    // The toaster is there on the start screen: the failed open says why there, and the screen stays.
    expect(await screen.findByText("That project isn't there any more.")).toBeInTheDocument();
    expect(startScreen()).not.toBeNull();
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Kiln vlog' })).toBeNull());
  });

  it('a_project_opened_from_the_start_screen_replaces_it_with_the_editor', async () => {
    rust.rows = [row('Studio tour', Date.now() - 60_000)];
    await launch();
    await screen.findByRole('button', { name: 'Studio tour' });
    // From the keyboard: Enter on the card. The card goes with the start screen, and the focus goes on to the
    // project's name in the top bar.
    act(() => card('Studio tour').focus());
    await userEvent.keyboard('{Enter}');
    expect(await screen.findByRole('main', { name: 'Preview' })).toBeInTheDocument();
    expect(startScreen()).toBeNull();
    expect(screen.getAllByRole('main')).toHaveLength(1);
    expect(rust.heard).toContain('project_open');
    const name = within(topBar()).getByRole('button', { name: 'Studio tour' });
    await waitFor(() => expect(name).toHaveFocus());
  });

  it('a_reload_of_the_page_shows_the_project_rust_has_open', async () => {
    rust.open = opened('Kiln vlog', 1);
    await launch();
    expect(await screen.findByRole('main', { name: 'Preview' })).toBeInTheDocument();
    expect(startScreen()).toBeNull();
    expect(within(topBar()).getByRole('button', { name: 'Kiln vlog' })).toBeInTheDocument();
    expect(rust.heard).not.toContain('projects_recent');
  });

  it('all_projects_saves_closes_and_shows_the_start_screen', async () => {
    rust.open = opened('Studio tour', 1);
    rust.rows = [row('Studio tour', Date.now() - 60_000)];
    const { useProject, useBurst, beginCommit } = await launch();
    await screen.findByRole('main', { name: 'Preview' });

    // A slider's change is still settling, behind a commit on its way, as All projects is chosen.
    const burst = renderHook(() => useBurst());
    const landed = beginCommit();
    act(() => burst.result.current.push({ label: 'text.color.undo', ops: [{ type: 'rename', name: 'Renamed' }] }));
    await userEvent.click(within(topBar()).getByRole('button', { name: 'Studio tour' }));
    await userEvent.click(await screen.findByRole('menuitem', { name: 'All projects' }));
    // The close waits for the change, so it is saved with the project.
    expect(rust.heard).not.toContain('project_close');
    rust.rows = [row('Studio tour', Date.now()), row('Older', Date.now() - 3_600_000)];
    landed();

    // Closed: the start screen, its list read again (the project just closed is there, with the time it was opened).
    expect(await screen.findByRole('main', { name: 'Your projects' })).toBeInTheDocument();
    const order = rust.heard.filter((cmd) => cmd === 'project_apply' || cmd === 'project_close');
    expect(order).toEqual(['project_apply', 'project_close']);
    expect(await screen.findByRole('button', { name: 'Older' })).toBeInTheDocument();
    expect(rust.heard.filter((cmd) => cmd === 'projects_recent')).toHaveLength(1);
    expect(editor()).toBeNull();
    expect(useProject.getState().snapshot).toBeNull();
    expect(within(topBar()).queryByRole('button', { name: /Undo/ })).toBeNull();
    burst.unmount();
  });

  it('all_projects_from_the_keyboard_puts_the_focus_on_the_title', async () => {
    rust.open = opened('Studio tour', 1);
    rust.rows = [row('Studio tour', Date.now() - 60_000)];
    await launch();
    await screen.findByRole('main', { name: 'Preview' });
    act(() => within(topBar()).getByRole('button', { name: 'Studio tour' }).focus());
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(screen.getByRole('menuitem', { name: 'New project' })).toHaveFocus());
    await userEvent.keyboard('{ArrowDown}');
    expect(screen.getByRole('menuitem', { name: 'All projects' })).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    // The menu's trigger went with the editor: the focus goes to the start screen's title, not to the page.
    const title = await screen.findByRole('heading', { name: 'Your projects' });
    await waitFor(() => expect(title).toHaveFocus());
    // Tab goes on from there, to New project.
    await screen.findByRole('button', { name: 'Studio tour' });
    await userEvent.tab();
    expect(within(startScreen()!).getByRole('button', { name: 'New project' })).toHaveFocus();
  });
});
