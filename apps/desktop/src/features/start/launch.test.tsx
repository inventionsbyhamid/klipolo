/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, render, screen, waitFor, within } from '@testing-library/react';
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
});

/** A project as Rust opens it: the fixture, under its own id and name. */
function opened(name: string, opening: number): Snapshot {
  const snapshot = fixtureSnapshot();
  return { ...snapshot, project: { ...snapshot.project, id: `id ${name}`, name }, opening };
}

/**
 * Rust as launch and the start screen see it: `open` is the project open now (none at launch), `rows` the library's
 * list; `holdApply`, when set, holds every edit's answer until it is called. Every command is kept in `heard`.
 */
const rust = {
  open: null as Snapshot | null,
  rows: [] as ProjectRow[],
  heard: [] as string[],
  openings: 0,
  holdApply: null as Promise<void> | null,
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
          await rust.holdApply;
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
 * own project store.
 */
async function launch() {
  vi.resetModules();
  fakeRust();
  const { App } = await import('../../App');
  const { useProject } = await import('../../state/project');
  render(<App />);
  return { useProject };
}

const startScreen = () => screen.queryByRole('main', { name: 'Your projects' });
const editor = () => screen.queryByRole('main', { name: 'Preview' });
const card = (name: string) => within(startScreen()!).getByRole('button', { name });
const topBar = () => document.querySelector<HTMLElement>('header.topbar')!;

afterEach(() => {
  clearMocks();
  Object.assign(rust, { open: null, rows: [], heard: [], openings: 0, holdApply: null });
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
    // Record your screen waits for the recorder: New project is the way on.
    expect(within(startScreen()!).getByRole('button', { name: 'New project' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Record your screen' })).toBeNull();
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
    // From the keyboard: Enter on the card.
    act(() => card('Studio tour').focus());
    await userEvent.keyboard('{Enter}');
    expect(await screen.findByRole('main', { name: 'Preview' })).toBeInTheDocument();
    expect(startScreen()).toBeNull();
    expect(screen.getAllByRole('main')).toHaveLength(1);
    expect(rust.heard).toContain('project_open');
    expect(within(topBar()).getByText('Studio tour')).toBeInTheDocument();
  });

  it('a_reload_of_the_page_shows_the_project_rust_has_open', async () => {
    rust.open = opened('Kiln vlog', 1);
    await launch();
    expect(await screen.findByRole('main', { name: 'Preview' })).toBeInTheDocument();
    expect(startScreen()).toBeNull();
    expect(within(topBar()).getByText('Kiln vlog')).toBeInTheDocument();
    expect(rust.heard).not.toContain('projects_recent');
  });
});
