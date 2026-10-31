/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, render, screen, waitFor } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixtureSnapshot } from '../../test/fixtures';

// The editor's panels stand in for themselves: these tests are about the recorder's place in the window.
vi.mock('../../layout/Library', () => ({ Library: () => <aside aria-label="Library" /> }));
vi.mock('../../preview/Preview', () => ({ Preview: () => <main aria-label="Preview" /> }));
vi.mock('../../features/timeline/Timeline', () => ({ Timeline: () => <section aria-label="Timeline" /> }));
vi.mock('../../layout/Chat', () => ({ Chat: () => <aside aria-label="Chat" /> }));
vi.mock('../../player/PlayerBar', () => ({ PlayerBar: () => <footer aria-label="Player" /> }));

/** A project as Rust opens it: the fixture, under its own id and name. */
function opened(name: string, opening: number): Snapshot {
  const snapshot = fixtureSnapshot();
  return { ...snapshot, project: { ...snapshot.project, id: `id ${name}`, name }, opening };
}

describe('in the window', () => {
  /** Rust as the window sees it: the project open now (none at launch), and every command heard. */
  const rust = { open: null as Snapshot | null, heard: [] as string[] };

  function fakeRust() {
    mockIPC(
      async (cmd, args: any) => {
        rust.heard.push(cmd);
        switch (cmd) {
          case 'project_snapshot':
            if (!rust.open) throw { code: 'project.none', params: {} };
            return rust.open;
          case 'projects_recent':
            return [];
          case 'project_create':
            rust.open = opened(args.name, 1);
            return rust.open;
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

  /** Klipolo launching (a fresh page) over `rust`, with a recorder that captures nothing. */
  async function launch() {
    vi.resetModules();
    fakeRust();
    const bridge = await import('../../test/recorderBridge');
    const fake = await bridge.installFakeRecorder();
    const { App } = await import('../../App');
    const stores = {
      useProject: (await import('../../state/project')).useProject,
      useRecorder: (await import('../../state/recorder')).useRecorder,
      useToasts: (await import('../../state/toasts')).useToasts,
    };
    render(<App />);
    return { fake, bridge, ...stores };
  }

  const recordDialog = () => screen.queryByRole('dialog', { name: 'Record' });
  const pressShiftCommandR = () => userEvent.keyboard('{Shift>}{Meta>}r{/Meta}{/Shift}');

  afterEach(() => {
    clearMocks();
    Object.assign(rust, { open: null, heard: [] });
    document.querySelectorAll('input[data-test-field]').forEach((field) => field.remove());
  });

  it('shift_command_r_opens_the_recorder_but_not_while_typing', async () => {
    rust.open = opened('Studio tour', 1);
    const { fake } = await launch();
    await screen.findByRole('main', { name: 'Preview' });
    // In a field, the keys are the field's.
    const field = document.createElement('input');
    field.dataset.testField = '';
    document.body.append(field);
    act(() => field.focus());
    await pressShiftCommandR();
    expect(recordDialog()).toBeNull();
    act(() => field.blur());
    // Command R or Shift R alone opens nothing.
    await userEvent.keyboard('{Meta>}r{/Meta}');
    await userEvent.keyboard('{Shift>}r{/Shift}');
    expect(recordDialog()).toBeNull();
    expect(fake.calls).not.toContain('setup');
    // Shift Command R: the Record dialog, its setup read once.
    await pressShiftCommandR();
    expect(await screen.findByRole('dialog', { name: 'Record' })).toBeInTheDocument();
    expect(fake.calls.filter((call) => call === 'setup')).toHaveLength(1);
  });

  it('shift_command_r_works_on_the_start_screen_but_never_over_another_dialog', async () => {
    await launch();
    await screen.findByRole('main', { name: 'Your projects' });
    // Settings is open (Command comma): Record waits for it to close.
    await userEvent.keyboard('{Meta>},{/Meta}');
    expect(await screen.findByRole('dialog', { name: 'Settings' })).toBeInTheDocument();
    await pressShiftCommandR();
    expect(recordDialog()).toBeNull();
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull());
    await pressShiftCommandR();
    expect(await screen.findByRole('dialog', { name: 'Record' })).toBeInTheDocument();
  });
});
