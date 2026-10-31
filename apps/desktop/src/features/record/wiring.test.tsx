/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, render, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CaptureEvent } from '../../bindings/CaptureEvent';
import type { Snapshot } from '../../bindings/Snapshot';
import type { WorkerState } from '../../bindings/WorkerState';
import { wordLabel } from '../../lib/labels';
import { beginCommit } from '../../state/commitQueue';
import { useProject } from '../../state/project';
import { useRecorder } from '../../state/recorder';
import { useSelection } from '../../state/selection';
import { useToasts } from '../../state/toasts';
import { fixtureSnapshot, revisionPatch } from '../../test/fixtures';
import {
  type FakeRecorder,
  type FakeRecorderScript,
  installFakeRecorder,
  recorderSetup,
  takeImport,
} from '../../test/recorderBridge';

// The editor's panels stand in for themselves: these tests are about the recorder's place in the window.
vi.mock('../../layout/Library', () => ({ Library: () => <aside aria-label="Library" /> }));
vi.mock('../../preview/Preview', () => ({ Preview: () => <main aria-label="Preview" /> }));
vi.mock('../../features/timeline/Timeline', () => ({ Timeline: () => <section aria-label="Timeline" /> }));
vi.mock('../../layout/Chat', () => ({ Chat: () => <aside aria-label="Chat" /> }));
vi.mock('../../player/PlayerBar', () => ({ PlayerBar: () => <footer aria-label="Player" /> }));

const S = 1_000_000;

// The worker's events, as the app forwards them.
const state = (s: WorkerState): CaptureEvent => ({ ev: 'state', state: s, elapsedUs: 0 });
const failed = (code: string): CaptureEvent => ({ ev: 'failed', error: { code, params: {} } });
const critical = (code: string): CaptureEvent => ({
  ev: 'health',
  note: { code, params: {} },
  level: 'critical',
  source: null,
});

/** A project as Rust opens it: the fixture, under its own id and name. */
function opened(name: string, opening: number): Snapshot {
  const snapshot = fixtureSnapshot();
  return { ...snapshot, project: { ...snapshot.project, id: `id ${name}`, name }, opening };
}

/**
 * `snapshot` once take `recordingId` landed in it: its recording (the fixture's, under that id) and its clip `clipId`,
 * `seconds` long, last on the main track, one step later; with its auto zoom's step on top when `zooms`, a zoom on the
 * first overlay track attached to the take.
 */
function withTake(snapshot: Snapshot, clipId: string, seconds: number, recordingId = 'rec', zooms = false): Snapshot {
  const { project, history } = snapshot;
  const [overlay, ...others] = project.timeline.tracks;
  const zoom = {
    ...overlay.items[0],
    id: `zoom-${clipId}`,
    attach: { clipId, source: { type: 'recording' as const, id: recordingId }, sourceUs: 0 },
    content: { type: 'zoom' as const, target: { type: 'auto' as const }, scale: 2 },
  };
  const tracks = zooms ? [{ ...overlay, items: [...overlay.items, zoom] }, ...others] : project.timeline.tracks;
  const recordings = project.recordings.some((r) => r.id === recordingId)
    ? project.recordings
    : [...project.recordings, { ...project.recordings[0], id: recordingId }];
  const clip = {
    ...project.timeline.main[0],
    id: clipId,
    source: { type: 'recording' as const, id: recordingId },
    sourceInUs: 0,
    sourceOutUs: seconds * S,
  };
  return {
    ...snapshot,
    project: { ...project, recordings, timeline: { ...project.timeline, tracks, main: [...project.timeline.main, clip] } },
    history: {
      ...history,
      revision: history.revision + (zooms ? 2 : 1),
      undoLabel: zooms ? 'undo.autoZoom' : 'undo.record',
      canUndo: true,
    },
  };
}

const toasts = () => useToasts.getState().toasts;
const texts = () => toasts().map((t) => t.text);

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
          case 'project_open':
            rust.open = opened(String(args.path).replace(/^.*\//, '').replace(/\.klipolo$/, ''), 2);
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

  /** Klipolo launching (a fresh page) over `rust`, with a recorder that captures nothing and answers as `script` says. */
  async function launch(script: FakeRecorderScript = {}) {
    vi.resetModules();
    fakeRust();
    const bridge = await import('../../test/recorderBridge');
    const fake = await bridge.installFakeRecorder(script);
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

  it('reopen_klipolo_comes_back_to_its_project_with_the_record_dialog', async () => {
    const file = '/Users/me/Movies/Klipolo/Studio tour.klipolo';
    const { fake, useProject } = await launch({ backFromReopen: () => ({ project: file }) });
    // Klipolo is back where the person was: their project, and Record.
    expect(await screen.findByRole('dialog', { name: 'Record' })).toBeInTheDocument();
    expect(useProject.getState().snapshot?.project.name).toBe('Studio tour');
    expect(screen.getByRole('main', { name: 'Preview' })).toBeInTheDocument();
    expect(rust.heard.filter((cmd) => cmd === 'project_open')).toHaveLength(1);
    expect(fake.calls.filter((call) => call === 'backFromReopen')).toHaveLength(1);
  });

  it('reopen_klipolo_from_the_start_screen_comes_back_to_the_record_dialog_there', async () => {
    await launch({ backFromReopen: () => ({ project: null }) });
    expect(await screen.findByRole('dialog', { name: 'Record' })).toBeInTheDocument();
    expect(screen.getByRole('main', { name: 'Your projects' })).toBeInTheDocument();
    expect(rust.heard).not.toContain('project_open');
  });

  it('an_ordinary_launch_opens_no_record_dialog', async () => {
    const { fake } = await launch();
    await screen.findByRole('main', { name: 'Your projects' });
    await waitFor(() => expect(fake.calls).toContain('backFromReopen'));
    expect(recordDialog()).toBeNull();
    expect(fake.calls).not.toContain('setup');
  });

  it('record_from_the_start_screen_lands_and_opens_the_new_project', async () => {
    const { fake, bridge, useProject, useRecorder, useToasts } = await launch();
    await userEvent.click(await screen.findByRole('button', { name: 'Record your screen' }));
    const dialog = await screen.findByRole('dialog', { name: 'Record' });
    await waitFor(() => expect(useRecorder.getState().setup).not.toBeNull());
    await userEvent.click(within(dialog).getByRole('button', { name: 'Record' }));
    await waitFor(() => expect(fake.calls).toContain('send:start'));
    act(() => fake.emit('take-1', state('recording')));
    // Klipolo's window hides for the take: so does the dialog.
    expect(recordDialog()).toBeNull();
    act(() => {
      fake.emit('take-1', state('stopping'));
      fake.emit('take-1', { ev: 'finished', session: 'take-1' });
    });
    // It lands in a new project that Rust made and opened: the editor shows it, the take's clip selected.
    const made = withTake(opened('Recording, Oct 31', 1), 'take-clip', 40, 'rec', true);
    act(() => fake.land(bridge.takeImport(made, 'take-clip', { recordingId: 'rec', zooms: 2 })));
    expect(await screen.findByRole('main', { name: 'Preview' })).toBeInTheDocument();
    expect(screen.queryByRole('main', { name: 'Your projects' })).toBeNull();
    expect(useProject.getState().snapshot?.project.name).toBe('Recording, Oct 31');
    const header = document.querySelector<HTMLElement>('header.topbar')!;
    expect(within(header).getByRole('button', { name: 'Recording, Oct 31' })).toBeInTheDocument();
    await waitFor(() =>
      expect(useToasts.getState().toasts.map((t) => t.text)).toContain(
        'Added Recording 1, 0:40. Zooms follow your clicks.',
      ),
    );
    act(() => fake.emit('take-1', state('idle')));
    expect(useRecorder.getState().open).toBe(false);
    // Rust made and opened the project: the page asked for nothing of the kind.
    expect(rust.heard.filter((cmd) => cmd === 'project_create' || cmd === 'project_open')).toEqual([]);
  });
});

describe('a take that lands', () => {
  let fake: FakeRecorder;
  const rec = () => useRecorder.getState();
  /** The project's undo calls, and what each found on top. */
  let undos: (string | null)[];

  beforeEach(async () => {
    undos = [];
    mockIPC((cmd) => {
      if (cmd === 'project_undo') {
        const now = useProject.getState().snapshot!;
        undos.push(now.history.undoLabel);
        return revisionPatch(now.history.revision + 1);
      }
      return null;
    });
    fake = await installFakeRecorder();
    useToasts.setState({ toasts: [] });
    useSelection.setState({ selectedId: null });
    useProject.setState({ snapshot: fixtureSnapshot() });
  });

  afterEach(() => {
    clearMocks();
    vi.restoreAllMocks();
  });

  /** Opens the dialog, presses Record, and gives the take it armed, recording. */
  async function recording(): Promise<string> {
    fake.script.setup = () => recorderSetup();
    await rec().openRecorder();
    await rec().record();
    const take = rec().take;
    if (take === null) throw new Error('Record armed no take');
    fake.emit(take, state('recording'));
    return take;
  }

  it('landing_toast_names_the_recording', async () => {
    const first = withTake(fixtureSnapshot(), 'take-clip', 40);
    fake.land(takeImport(first, 'take-clip', { recordingId: 'rec' }));
    await vi.waitFor(() => expect(texts()).toEqual(['Added Recording 1, 0:40.']));
    expect(toasts()[0]).toMatchObject({ kind: 'info', done: true });
    expect(toasts()[0].actions).toBeUndefined();
    expect(useSelection.getState().selectedId).toBe('take-clip');
    // The project's second recording, with zooms: its toast takes the first one's place, with Remove zooms.
    const second = withTake(first, 'take-2', 61.4, 'rec-2', true);
    fake.land(takeImport(second, 'take-2', { recordingId: 'rec-2', zooms: 3 }));
    await vi.waitFor(() => expect(texts()).toEqual(['Added Recording 2, 1:01. Zooms follow your clicks.']));
    expect(toasts()[0].actions?.map((a) => a.label)).toEqual(['Remove zooms']);
  });

  it('remove_zooms_undoes_only_the_zoom_step', async () => {
    const landed = withTake(fixtureSnapshot(), 'take-clip', 40, 'rec', true);
    fake.land(takeImport(landed, 'take-clip', { recordingId: 'rec', zooms: 2 }));
    await vi.waitFor(() => expect(toasts()[0]?.actions).toHaveLength(1));
    const remove = toasts()[0].actions![0];
    act(() => remove.run());
    await vi.waitFor(() => expect(undos).toEqual(['undo.autoZoom']));
    // A hand edit on top: Remove zooms takes nothing back, and says how to remove them.
    const now = useProject.getState().snapshot!;
    useProject.setState({ snapshot: { ...now, history: { ...now.history, undoLabel: 'undo.split.clip' } } });
    act(() => remove.run());
    await vi.waitFor(() =>
      expect(texts()).toContain("Klipolo can't remove them now. Delete them on the timeline."),
    );
    expect(undos).toEqual(['undo.autoZoom']);
  });

  it('remove_zooms_after_an_undo_says_the_zooms_are_already_gone', async () => {
    const landed = withTake(fixtureSnapshot(), 'take-clip', 40, 'rec', true);
    fake.land(takeImport(landed, 'take-clip', { recordingId: 'rec', zooms: 2 }));
    await vi.waitFor(() => expect(toasts()[0]?.actions).toHaveLength(1));
    // The person pressed Undo themselves: the zooms went, and "Add recording" is on top.
    const undone = withTake(fixtureSnapshot(), 'take-clip', 40, 'rec');
    useProject.setState({
      snapshot: { ...undone, history: { ...undone.history, canRedo: true, redoLabel: 'undo.autoZoom' } },
    });
    act(() => toasts()[0].actions![0].run());
    await vi.waitFor(() => expect(texts()).toContain('The zooms are already gone.'));
    expect(texts()).not.toContain("Klipolo can't remove them now. Delete them on the timeline.");
    expect(undos).toEqual([]);
  });

  it('remove_zooms_never_undoes_in_another_project', async () => {
    const landed = withTake(fixtureSnapshot(), 'take-clip', 40, 'rec', true);
    fake.land(takeImport(landed, 'take-clip', { recordingId: 'rec', zooms: 2 }));
    await vi.waitFor(() => expect(toasts()[0]?.actions).toHaveLength(1));
    // Another project is open now, its own last change an auto zoom too.
    const other = withTake(opened('Other', 2), 'other-clip', 10, 'rec', true);
    useProject.setState({ snapshot: other });
    act(() => toasts()[0].actions![0].run());
    await vi.waitFor(() =>
      expect(texts()).toContain("Klipolo can't remove them now. Delete them on the timeline."),
    );
    expect(undos).toEqual([]);
  });

  it('undo_label_reads_add_auto_zoom', () => {
    expect(wordLabel('undo.autoZoom')).toBe('Add auto zoom');
    expect(wordLabel('undo.record')).toBe('Add recording');
  });

  it('a_take_landing_in_another_opening_lands_the_shown_project_s_edits_first', async () => {
    for (const [name, opening] of [
      ['Armed', 2],
      ['Billing tutorial', 7],
    ] as const) {
      // Another project (the one the take was armed in, opened again), or the one shown opened again.
      const shown = { ...fixtureSnapshot(), opening: 1 };
      useProject.setState({ snapshot: shown });
      const base = name === 'Armed' ? opened(name, opening) : { ...fixtureSnapshot(), opening };
      const arrived = withTake(base, 'take-clip', 40);
      // A hand edit is still on its way to Rust.
      const sent = beginCommit();
      fake.land(takeImport(arrived, 'take-clip', { recordingId: 'rec' }));
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(useProject.getState().snapshot).toBe(shown);
      sent();
      await vi.waitFor(() => expect(useProject.getState().snapshot?.opening).toBe(opening));
      expect(useProject.getState().snapshot?.project.id).toBe(arrived.project.id);
      await vi.waitFor(() => expect(useSelection.getState().selectedId).toBe('take-clip'));
    }
  });

  it('a_landed_take_s_notes_are_said_once', async () => {
    const landed = withTake(fixtureSnapshot(), 'take-clip', 40);
    fake.land(
      takeImport(landed, 'take-clip', {
        recordingId: 'rec',
        notes: [
          { code: 'capture.note.savedFromFiles', params: {} },
          // Said already as Klipolo came back from the take.
          { code: 'capture.note.cameraDropped', params: {} },
        ],
      }),
    );
    await vi.waitFor(() =>
      expect(texts()).toEqual([
        'Added Recording 1, 0:40.',
        'The recorder stopped while saving, so Klipolo finished the recording from its files.',
      ]),
    );
  });

  it('added_not_opened_is_a_note_not_a_failure', async () => {
    const take = await recording();
    fake.emit(take, state('stopping'));
    fake.emit(take, { ev: 'finished', session: take });
    expect(rec().open).toBe(false);
    // The landing couldn't open the project after adding the take.
    fake.emit(take, critical('capture.import.addedNotOpened'));
    fake.emit(take, state('idle'));
    expect(toasts().map((t) => [t.kind, t.text])).toEqual([
      ['info', 'The recording was added to your project. Open the project from the start screen to see it.'],
    ]);
    expect(rec().error).toBeNull();

    // A take finished from its files (no `finished`) says it as it ends, the same way.
    useToasts.setState({ toasts: [] });
    const next = await recording();
    fake.emit(next, critical('capture.import.addedNotOpened'));
    fake.emit(next, state('idle'));
    expect(toasts().map((t) => t.kind)).toEqual(['info']);
  });

  it('a_take_that_was_not_added_says_why_once_klipolo_is_back', async () => {
    const take = await recording();
    fake.emit(take, state('stopping'));
    fake.emit(take, { ev: 'finished', session: take });
    fake.emit(take, critical('capture.import.notAdded'));
    fake.emit(take, state('idle'));
    expect(toasts().map((t) => [t.kind, t.text])).toEqual([
      ['error', "The recording couldn't be added to your project. Its files are in Movies / Klipolo / Recordings."],
    ]);
    // Its worker has gone: a later note of that take is no one's.
    fake.emit(take, critical('capture.import.notAdded'));
    expect(toasts()).toHaveLength(1);
  });

  it('a_take_given_up_while_saving_still_lands', async () => {
    const take = await recording();
    fake.emit(take, state('stopping'));
    // The drive stopped answering while it saved: the worker gives up, and Klipolo finishes it from its files.
    fake.emit(take, failed('capture.error.driveStuck'));
    expect(rec().open).toBe(false);
    expect(texts()).toEqual([
      'The drive stopped answering. Klipolo finishes the recording from what was saved. Check the drive before you record again.',
    ]);
    fake.land(takeImport(withTake(fixtureSnapshot(), 'take-clip', 40), 'take-clip', { take, recordingId: 'rec' }));
    await vi.waitFor(() => expect(texts()).toContain('Added Recording 1, 0:40.'));
    expect(useProject.getState().snapshot?.project.timeline.main.at(-1)?.id).toBe('take-clip');
    fake.emit(take, state('idle'));
    expect(rec().error).toBeNull();
  });
});
