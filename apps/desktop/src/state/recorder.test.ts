import { emit } from '@tauri-apps/api/event';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ArmedTake } from '../bindings/ArmedTake';
import type { ArmOptions } from '../bindings/ArmOptions';
import type { CaptureEvent } from '../bindings/CaptureEvent';
import type { Snapshot } from '../bindings/Snapshot';
import type { WorkerState } from '../bindings/WorkerState';
import { tauriRecorder } from '../bridge/recorder';
import { deviceDenied, screenStage, screenStep } from '../lib/recorder/permission';
import { phaseOf } from '../lib/recorder/phase';
import { clipStarts } from '../lib/timeline';
import { fixtureSnapshot } from '../test/fixtures';
import {
  deferred,
  device,
  display,
  type FakeRecorder,
  installFakeRecorder,
  recorderSetup,
  refusal,
  takeImport,
} from '../test/recorderBridge';
import { usePlayback } from './playback';
import { useProject } from './project';
import { REOPEN_GRACE_MS, useRecorder } from './recorder';
import { useSelection } from './selection';
import { useToasts } from './toasts';

const rec = () => useRecorder.getState();
const phase = () => phaseOf(rec());

// The worker's events, as the app forwards them.
const state = (s: WorkerState): CaptureEvent => ({ ev: 'state', state: s, elapsedUs: 0 });
const countdown = (leftS: number): CaptureEvent => ({ ev: 'countdown', leftS });
const gate = (...missing: string[]): CaptureEvent => ({ ev: 'gate', missing });
const meter = (source: string, peakDbfs: number): CaptureEvent => ({ ev: 'meter', source, peakDbfs });
const failed = (code: string): CaptureEvent => ({ ev: 'failed', error: { code, params: {} } });

let fake: FakeRecorder;

beforeEach(async () => {
  fake = await installFakeRecorder();
  useToasts.setState({ toasts: [] });
  useSelection.setState({ selectedId: null });
});

afterEach(() => {
  vi.restoreAllMocks();
});

/** Opens the dialog on `setup` and waits for it to be read. */
async function opened(setup = recorderSetup()) {
  fake.script.setup = () => setup;
  await rec().openRecorder();
}

/** Opens the dialog, presses Record, and gives the take it armed. */
async function pressed(setup = recorderSetup()): Promise<string> {
  await opened(setup);
  await rec().record();
  const take = rec().take;
  if (take === null) throw new Error('Record armed no take');
  return take;
}

/** `snapshot` with the take's clip added at the end of the main track, one step later. */
function landed(snapshot: Snapshot, clipId: string): Snapshot {
  const { project, history } = snapshot;
  const main = project.timeline.main;
  return {
    ...snapshot,
    project: { ...project, timeline: { ...project.timeline, main: [...main, { ...main[0], id: clipId }] } },
    history: { ...history, revision: history.revision + 1, undoLabel: 'undo.record' },
  };
}

describe('opening', () => {
  it('opening_reads_setup_and_picks_the_first_display_and_default_mic', async () => {
    fake.script.setup = () =>
      recorderSetup({
        displays: [display('built-in'), display('studio')],
        mics: [device('usb'), device('built-in mic')],
        mic: 'built-in mic',
      });
    const reading = rec().openRecorder();
    expect(rec().open).toBe(true);
    expect(phase()).toBe('loading');
    await reading;
    expect(phase()).toBe('choosing');
    expect(rec().picks).toEqual({
      kind: 'screen',
      display: 'built-in',
      appId: null,
      mic: 'built-in mic',
      camera: null,
      showCamera: true,
      mirror: true,
    });
    // Opening reads the setup: it arms, asks and captures nothing.
    expect(fake.calls).toEqual(['setup']);
  });

  it('a_refused_setup_says_why_and_try_again_reads_it_again', async () => {
    fake.script.setup = () => refusal('capture.error.testRun');
    await rec().openRecorder();
    expect(phase()).toBe('unavailable');
    expect(rec().setupError).toBe('Recording is off in this test run.');
    fake.script.setup = () => recorderSetup();
    await rec().openRecorder();
    expect(phase()).toBe('choosing');
    expect(rec().setupError).toBeNull();
    // Open with its setup read: opening again reads nothing.
    await rec().openRecorder();
    expect(fake.calls).toEqual(['setup', 'setup']);
  });

  it('setup_is_read_once_per_opening_and_never_twice_at_once', async () => {
    const first = deferred<ReturnType<typeof recorderSetup>>();
    fake.script.setup = () => first.promise;
    const opening = rec().openRecorder();
    // Closed and opened again while the first read is on its way: the new opening waits for that read.
    rec().close();
    const again = rec().openRecorder();
    expect(fake.calls).toEqual(['setup']);
    first.resolve(recorderSetup({ displays: [display('studio')] }));
    await Promise.all([opening, again]);
    expect(rec().picks.display).toBe('studio');
    // A later opening reads it again.
    rec().close();
    await rec().openRecorder();
    expect(fake.calls).toEqual(['setup', 'setup']);
  });

  it('a_second_opening_while_loading_resolves_once_the_setup_is_read', async () => {
    const read = deferred<ReturnType<typeof recorderSetup>>();
    fake.script.setup = () => read.promise;
    const first = rec().openRecorder();
    let done = false;
    const second = rec()
      .openRecorder()
      .then(() => {
        done = true;
      });
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(done).toBe(false);
    read.resolve(recorderSetup());
    await Promise.all([first, second]);
    expect(phase()).toBe('choosing');
    expect(fake.calls).toEqual(['setup']);
  });

  it('two_displays_offer_both', async () => {
    await opened(recorderSetup({ displays: [display('built-in'), display('studio')] }));
    expect(rec().setup?.devices.displays.map((d) => d.id)).toEqual(['built-in', 'studio']);
    expect(rec().picks.display).toBe('built-in');
    rec().pick({ display: 'studio' });
    await rec().record();
    expect(fake.armed.map((o) => o.what)).toEqual([{ kind: 'screen', display: 'studio' }]);
  });
});

describe('record', () => {
  it('record_arms_then_sends_start_once', async () => {
    await opened();
    const first = rec().record();
    // A double press.
    const second = rec().record();
    expect(phase()).toBe('starting');
    await Promise.all([first, second]);
    expect(fake.calls).toEqual(['setup', 'arm', 'send:start']);
    expect(fake.sent).toEqual([{ take: 'take-1', command: { cmd: 'start' } }]);
    expect(rec().take).toBe('take-1');
    // Pressed again while it counts down: still one take.
    await rec().record();
    expect(fake.armed).toHaveLength(1);
  });

  it('record_sends_the_choices_as_the_take_s_options', async () => {
    await opened(recorderSetup({ cameras: [device('facetime')] }));
    rec().pick({ camera: 'facetime', mirror: false });
    await rec().record();
    expect(fake.armed).toEqual<ArmOptions[]>([
      {
        what: { kind: 'screen', display: 'main' },
        mic: 'mic-1',
        camera: 'facetime',
        cameraBubble: true,
        mirrorCamera: false,
      },
    ]);
  });

  it('one_app_records_nothing_until_an_app_is_chosen', async () => {
    await opened();
    rec().pick({ kind: 'app' });
    await rec().record();
    expect(fake.calls).toEqual(['setup']);
    rec().pick({ appId: 'com.apple.Notes' });
    await rec().record();
    expect(fake.armed[0].what).toEqual({ kind: 'app', appId: 'com.apple.Notes', display: 'main' });
  });

  it('screen_not_granted_shows_the_step_and_record_does_nothing', async () => {
    for (const screen of ['notAsked', 'denied', 'unknown'] as const) {
      fake = await installFakeRecorder();
      await opened(recorderSetup({ permissions: { screen } }));
      const s = rec();
      expect(s.setup && screenStep(s.setup, s.picks, s.screenRefused)).toBe(true);
      await rec().record();
      expect(fake.calls).toEqual(['setup']);
      expect(phase()).toBe('choosing');
    }
  });

  it('a_take_refused_the_screen_shows_the_step_again', async () => {
    const take = await pressed();
    fake.emit(take, failed('capture.permission.screen'));
    const s = rec();
    expect(s.screenRefused).toBe(true);
    expect(s.setup && screenStep(s.setup, s.picks, s.screenRefused)).toBe(true);
    fake.emit(take, state('idle'));
    await rec().record();
    expect(fake.armed).toHaveLength(1);
  });

  it('allow_then_reopen_then_still_refused_stages', async () => {
    const now = vi.spyOn(Date, 'now').mockReturnValue(1_000_000);
    await opened(recorderSetup({ permissions: { screen: 'notAsked' } }));
    const stage = () => screenStage(rec().setup ?? recorderSetup(), rec().askedScreen);
    expect(stage()).toBe('ask');
    await rec().allowScreen();
    expect(rec().askedScreen).toBe(true);
    expect(stage()).toBe('reopen');
    // The second click of a double click on Allow lands on Reopen, which took its place: Klipolo stays.
    await rec().reopen();
    expect(fake.calls).toEqual(['setup', 'allowScreen']);
    now.mockReturnValue(1_000_000 + REOPEN_GRACE_MS);
    await rec().reopen();
    expect(fake.calls).toEqual(['setup', 'allowScreen', 'reopen']);

    // Klipolo has reopened, and macOS still says no.
    fake = await installFakeRecorder();
    await opened(recorderSetup({ permissions: { screen: 'denied' }, screenRefusedAfterReopen: true }));
    expect(rec().askedScreen).toBe(false);
    expect(stage()).toBe('stillRefused');
    await rec().openPrivacy('screen');
    await rec().allowScreen();
    expect(stage()).toBe('reopen');
    expect(fake.calls).toEqual(['setup', 'openPrivacy:screen', 'allowScreen']);
  });

  it('a_refused_permission_press_says_why', async () => {
    await opened(recorderSetup({ permissions: { screen: 'notAsked' } }));
    fake.script.reopen = () => refusal('capture.busy', { place: { code: 'capture.place.menuBar', params: {} } });
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000 + REOPEN_GRACE_MS * 10);
    await rec().reopen();
    expect(rec().error).toMatch(/^A recording is already running\./);
  });

  it('denied_mic_is_flagged_and_no_microphone_still_records', async () => {
    await opened(recorderSetup({ mics: [device('usb')], permissions: { mic: 'denied' } }));
    expect(rec().picks.mic).toBe('usb');
    expect(deviceDenied(rec().setup ?? recorderSetup(), 'mic', rec().picks)).toBe(true);
    await rec().openPrivacy('mic');
    rec().pick({ mic: null });
    expect(deviceDenied(rec().setup ?? recorderSetup(), 'mic', rec().picks)).toBe(false);
    await rec().record();
    expect(fake.armed[0]).toMatchObject({ mic: null, camera: null });
    expect(fake.calls).toEqual(['setup', 'openPrivacy:mic', 'arm', 'send:start']);
  });

  it('arming_refused_says_why_and_the_choices_come_back', async () => {
    await opened();
    fake.script.arm = () => refusal('capture.error.testRun');
    await rec().record();
    expect(phase()).toBe('choosing');
    expect(rec().error).toBe('Recording is off in this test run.');
    expect(fake.sent).toEqual([]);
  });

  it('a_refused_start_says_why_and_lets_the_armed_take_go', async () => {
    await opened();
    fake.script.send = (_take, command) =>
      command.cmd === 'start' ? refusal('capture.error.unsupported') : undefined;
    await rec().record();
    expect(phase()).toBe('choosing');
    expect(rec().error).toBe("This computer can't record that.");
    expect(fake.sent.map((s) => s.command.cmd)).toEqual(['start', 'disarm']);
  });
});

describe('the take', () => {
  it('countdown_meter_and_gate_follow_events', async () => {
    const take = await pressed();
    fake.emit(take, state('countdown'));
    fake.emit(take, countdown(3));
    fake.emit(take, meter('mic', -18));
    expect(rec()).toMatchObject({ worker: 'countdown', countdown: 3, meterDbfs: -18, gate: null });
    // Only the microphone moves the meter.
    fake.emit(take, meter('screen', -3));
    expect(rec().meterDbfs).toBe(-18);
    // The gate holds the countdown: its question stays until the countdown moves on.
    fake.emit(take, gate('mic'));
    fake.emit(take, countdown(3));
    expect(rec().gate).toEqual(['mic']);
    fake.emit(take, countdown(3));
    expect(rec().gate).toEqual(['mic']);
    fake.emit(take, countdown(2));
    expect(rec()).toMatchObject({ countdown: 2, gate: null });
    fake.emit(take, state('recording'));
    expect(rec()).toMatchObject({ worker: 'recording', countdown: null, gate: null, meterDbfs: null });
    expect(phase()).toBe('recording');
  });

  it('record_without_it_sends_without_source', async () => {
    const take = await pressed();
    fake.emit(take, countdown(3));
    fake.emit(take, gate('mic', 'camera'));
    await rec().recordWithout('mic');
    expect(fake.sent.at(-1)).toEqual({ take, command: { cmd: 'withoutSource', source: 'mic' } });
    expect(rec().gate).toEqual(['camera']);
    await rec().recordWithout('camera');
    expect(rec().gate).toBeNull();
  });

  it('events_for_another_take_are_ignored', async () => {
    const take = await pressed();
    fake.emit('take-0', countdown(3));
    fake.emit('take-0', failed('capture.error.encoder'));
    fake.emit('take-0', state('idle'));
    expect(rec()).toMatchObject({ take, countdown: null, error: null });
    expect(phase()).toBe('starting');
    fake.emit(take, countdown(3));
    expect(rec().countdown).toBe(3);
  });

  it('failure_or_idle_returns_to_choosing_with_the_sentence', async () => {
    const take = await pressed();
    fake.emit(take, countdown(2));
    fake.emit(take, failed('capture.error.encoder'));
    expect(phase()).toBe('choosing');
    expect(rec()).toMatchObject({ take: null, countdown: null, error: 'The video encoder stopped.' });
    // The take's last word, once its worker has gone: the sentence stays.
    fake.emit(take, state('idle'));
    expect(rec().error).toBe('The video encoder stopped.');
    // A take that ends with nothing to save: the choices come back.
    await rec().record();
    const next = rec().take;
    expect(next).toBe('take-2');
    expect(rec().error).toBeNull();
    fake.emit('take-2', state('idle'));
    expect(phase()).toBe('choosing');
    expect(rec().take).toBeNull();
  });

  it('cancel_in_the_countdown_cancels_the_take_and_the_next_waits_for_it_to_go', async () => {
    const take = await pressed();
    fake.emit(take, countdown(3));
    fake.script.idle = false;
    await rec().cancel();
    expect(fake.sent.at(-1)).toEqual({ take, command: { cmd: 'cancel' } });
    expect(phase()).toBe('choosing');
    expect(rec().take).toBeNull();
    // Its late words aren't the dialog's.
    fake.emit(take, countdown(2));
    expect(rec().countdown).toBeNull();
    // Record waits for its worker to go, so the app is free for the next take.
    const next = rec().record();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fake.armed).toHaveLength(1);
    fake.emit(take, state('idle'));
    await next;
    expect(fake.armed).toHaveLength(2);
    expect(rec().take).toBe('take-2');
  });

  it('cancel_while_arming_lets_the_take_go_and_it_never_starts', async () => {
    await opened();
    const arming = deferred<ArmedTake>();
    fake.script.arm = () => arming.promise;
    const pressing = rec().record();
    await vi.waitFor(() => expect(fake.calls).toContain('arm'));
    await rec().cancel();
    expect(phase()).toBe('choosing');
    arming.resolve({ id: 'take-1', countdownS: 3 });
    await pressing;
    expect(fake.sent).toEqual([{ take: 'take-1', command: { cmd: 'disarm' } }]);
    expect(rec().take).toBeNull();
  });

  it('closing_in_the_countdown_cancels_the_take', async () => {
    const take = await pressed();
    fake.emit(take, countdown(3));
    rec().close();
    expect(rec().open).toBe(false);
    await vi.waitFor(() => expect(fake.sent.at(-1)).toEqual({ take, command: { cmd: 'cancel' } }));
  });

  it('closing_right_after_record_disarms_the_take_once_it_is_armed', async () => {
    await opened();
    const arming = deferred<ArmedTake>();
    fake.script.arm = () => arming.promise;
    const pressing = rec().record();
    await vi.waitFor(() => expect(fake.calls).toContain('arm'));
    rec().close();
    expect(rec().open).toBe(false);
    arming.resolve({ id: 'take-1', countdownS: 3 });
    await pressing;
    expect(fake.sent).toEqual([{ take: 'take-1', command: { cmd: 'disarm' } }]);
    expect(rec().take).toBeNull();
  });

  it('closing_while_recording_saves_the_take', async () => {
    const take = await pressed();
    fake.emit(take, state('recording'));
    rec().close();
    expect(rec().open).toBe(false);
    await vi.waitFor(() => expect(fake.sent.at(-1)).toEqual({ take, command: { cmd: 'stop' } }));
    // Shift Command R during the take: nothing new shows, nothing changes.
    await rec().openRecorder();
    expect(rec()).toMatchObject({ open: false, take, worker: 'recording' });
    expect(fake.calls.filter((c) => c === 'setup')).toHaveLength(1);
    // The take is followed to its end: saved, and the dialog stays closed.
    fake.emit(take, state('stopping'));
    expect(rec().worker).toBe('stopping');
    fake.emit(take, { ev: 'finished', session: take });
    expect(rec()).toMatchObject({ open: false, take: null });
    fake.emit(take, state('idle'));
    await rec().openRecorder();
    expect(rec().open).toBe(true);
  });

  it('a_take_given_up_after_recording_closes_the_dialog_when_klipolo_is_back', async () => {
    const take = await pressed();
    fake.emit(take, state('recording'));
    // The worker kept crashing: the app finishes the take from its files and frees it, with no `finished`.
    fake.emit(take, {
      ev: 'health',
      note: { code: 'capture.stop.crashLoop', params: {} },
      level: 'critical',
      source: null,
    });
    fake.emit(take, state('idle'));
    expect(rec()).toMatchObject({ open: false, take: null, error: null });
    expect(useToasts.getState().toasts.map((t) => t.text)).toEqual([
      'The recorder kept stopping, so the recording was saved.',
    ]);
  });

  it('a_take_finished_from_its_files_while_saving_closes_the_dialog_when_klipolo_is_back', async () => {
    const take = await pressed();
    fake.emit(take, state('recording'));
    fake.emit(take, state('stopping'));
    // Its worker died while saving: the app finishes it from its files, then frees it.
    fake.emit(take, state('idle'));
    expect(rec()).toMatchObject({ open: false, take: null });
    expect(useToasts.getState().toasts).toEqual([]);
  });

  it('a_take_that_fails_after_recording_closes_the_dialog_and_says_why_once', async () => {
    const take = await pressed();
    fake.emit(take, state('recording'));
    fake.emit(take, failed('capture.error.encoder'));
    expect(rec()).toMatchObject({ open: false, take: null });
    fake.emit(take, state('idle'));
    expect(useToasts.getState().toasts.map((t) => t.text)).toEqual(['The video encoder stopped.']);
  });

  it('a_take_stopped_in_its_countdown_comes_back_to_the_choices', async () => {
    const take = await pressed();
    fake.emit(take, countdown(3));
    // Stopped from the menu bar before the gate: the worker says `stopping` too, and nothing was recorded.
    fake.emit(take, state('stopping'));
    fake.emit(take, state('idle'));
    expect(rec().open).toBe(true);
    expect(phase()).toBe('choosing');
    expect(useToasts.getState().toasts).toEqual([]);
  });

  it('a_refused_cancel_falls_back_to_stop_the_running_recording', async () => {
    const take = await pressed();
    fake.emit(take, countdown(3));
    fake.script.send = (_take, command) =>
      command.cmd === 'cancel' ? refusal('capture.error.unsupported') : undefined;
    await rec().cancel();
    expect(fake.calls.slice(-2)).toEqual(['send:cancel', 'stopRunning']);
    expect(rec().error).toBeNull();
    // Stop couldn't be sent either: that is said.
    fake = await installFakeRecorder();
    const next = await pressed();
    fake.emit(next, countdown(3));
    fake.script.send = (_take, command) =>
      command.cmd === 'cancel' ? refusal('capture.error.unsupported') : undefined;
    fake.script.stopRunning = () => refusal('capture.error.testRun');
    await rec().cancel();
    expect(rec().error).toBe('Recording is off in this test run.');
  });

  it('a_late_refused_cancel_never_says_its_sentence_on_the_next_press', async () => {
    const take = await pressed();
    fake.emit(take, countdown(3));
    const answer = deferred<void>();
    fake.script.send = (_take, command) => (command.cmd === 'cancel' ? answer.promise : undefined);
    fake.script.stopRunning = () => refusal('capture.error.testRun');
    const cancelling = rec().cancel();
    // Its worker goes anyway, and the person presses Record again.
    fake.emit(take, state('idle'));
    await rec().record();
    expect(rec().take).toBe('take-2');
    answer.reject(refusal('capture.error.unsupported'));
    await cancelling;
    expect(rec().error).toBeNull();
  });

  it('a_take_that_fails_before_arming_answers_never_starts', async () => {
    await opened();
    const arming = deferred<ArmedTake>();
    fake.script.arm = () => arming.promise;
    const pressing = rec().record();
    await vi.waitFor(() => expect(fake.calls).toContain('arm'));
    // Its worker speaks before arming has said its id.
    fake.emit('take-1', failed('capture.error.encoder'));
    arming.resolve({ id: 'take-1', countdownS: 3 });
    await pressing;
    expect(fake.sent).toEqual([]);
    expect(phase()).toBe('choosing');
    expect(rec().error).toBe('The video encoder stopped.');
  });

  it('a_critical_note_is_said_once_klipolo_is_back', async () => {
    const take = await pressed();
    fake.emit(take, state('recording'));
    fake.emit(take, {
      ev: 'health',
      note: { code: 'capture.stop.encoder', params: {} },
      level: 'critical',
      source: 'screen',
    });
    fake.emit(take, {
      ev: 'health',
      note: { code: 'capture.error.busy', params: {} },
      level: 'warning',
      source: 'camera',
    });
    // Klipolo's window is hidden while the take records: nothing is said yet.
    expect(useToasts.getState().toasts).toEqual([]);
    fake.emit(take, state('stopping'));
    expect(phase()).toBe('saving');
    fake.emit(take, { ev: 'finished', session: take });
    expect(useToasts.getState().toasts.map((t) => t.text)).toEqual([
      'The video encoder stopped, so the recording stopped and was saved.',
    ]);
    expect(rec().open).toBe(false);
    expect(rec().note).toBeNull();
    // Said once.
    fake.emit(take, state('idle'));
    expect(useToasts.getState().toasts).toHaveLength(1);
  });

  it('a_camera_that_stops_mid_take_is_said_once_klipolo_is_back', async () => {
    const take = await pressed(recorderSetup({ cameras: [device('facetime')] }));
    fake.emit(take, state('recording'));
    // The worker gives the camera up ("Camera disconnected."), then the app says what that means.
    fake.emit(take, {
      ev: 'health',
      note: { code: 'capture.health.lost', params: { source: 'camera', reason: 'encoder' } },
      level: 'warning',
      source: 'camera',
    });
    expect(rec().note).toBeNull();
    fake.emit(take, {
      ev: 'health',
      note: { code: 'capture.note.cameraDropped', params: {} },
      level: 'warning',
      source: 'camera',
    });
    expect(rec().note).not.toBeNull();
    const camera = rec().note;
    // A critical note outranks it; a later warning doesn't replace a critical one.
    fake.emit(take, {
      ev: 'health',
      note: { code: 'capture.stop.crashLoop', params: {} },
      level: 'critical',
      source: null,
    });
    fake.emit(take, {
      ev: 'health',
      note: { code: 'capture.note.cameraDropped', params: {} },
      level: 'warning',
      source: 'camera',
    });
    expect(rec().note).toBe('The recorder kept stopping, so the recording was saved.');
    expect(rec().note).not.toBe(camera);
    expect(useToasts.getState().toasts).toEqual([]);
    fake.emit(take, { ev: 'finished', session: take });
    expect(useToasts.getState().toasts.map((t) => t.text)).toEqual([
      'The recorder kept stopping, so the recording was saved.',
    ]);
  });

  it('imported_take_loads_the_project_and_selects_its_clip', async () => {
    const before = fixtureSnapshot();
    useProject.setState({ snapshot: before });
    fake.land(takeImport(landed(before, 'take-clip'), 'take-clip'));
    await vi.waitFor(() => expect(useSelection.getState().selectedId).toBe('take-clip'));
    const main = useProject.getState().snapshot?.project.timeline.main ?? [];
    expect(main.at(-1)?.id).toBe('take-clip');
    expect(usePlayback.getState().playheadUs).toBe(clipStarts(main).at(-1));
  });

  it('a_take_from_the_start_screen_opens_its_new_project', async () => {
    useProject.setState({ snapshot: null });
    const made = landed(fixtureSnapshot(), 'take-clip');
    fake.land(takeImport(made, 'take-clip'));
    await vi.waitFor(() => expect(useSelection.getState().selectedId).toBe('take-clip'));
    expect(useProject.getState().snapshot?.project.id).toBe(made.project.id);
  });
});

describe('the bridge', () => {
  afterEach(() => clearMocks());

  it('the_bridge_calls_the_app_s_recorder_commands', async () => {
    const calls: [string, unknown][] = [];
    mockIPC((cmd, args) => {
      calls.push([cmd, args]);
      return cmd === 'recorder_arm' ? { id: 'take-1', countdownS: 3 } : null;
    });
    const options: ArmOptions = {
      what: { kind: 'screen', display: 'main' },
      mic: null,
      camera: null,
      cameraBubble: false,
      mirrorCamera: true,
    };
    await tauriRecorder.setup();
    expect(await tauriRecorder.arm(options)).toEqual({ id: 'take-1', countdownS: 3 });
    await tauriRecorder.send('take-1', { cmd: 'start' });
    await tauriRecorder.stopRunning();
    await tauriRecorder.allowScreen();
    await tauriRecorder.reopen();
    await tauriRecorder.openPrivacy('camera');
    expect(calls.map(([cmd]) => cmd)).toEqual([
      'recorder_setup',
      'recorder_arm',
      'recorder_send',
      'recorder_stop_running',
      'recorder_allow_screen',
      'recorder_reopen',
      'recorder_open_privacy',
    ]);
    expect(calls[1][1]).toEqual({ options });
    expect(calls[2][1]).toEqual({ take: 'take-1', command: { cmd: 'start' } });
    expect(calls[6][1]).toEqual({ page: 'camera' });
  });

  it('the_bridge_hears_the_recorder_and_the_landing', async () => {
    mockIPC(() => null, { shouldMockEvents: true });
    const heard: unknown[] = [];
    const offEvents = await tauriRecorder.onEvent((e) => heard.push(e));
    const offLanded = await tauriRecorder.onImported((t) => heard.push(t.clipId));
    await emit('recorder-event', { take: 'take-1', event: countdown(3) });
    await emit('recorder-imported', takeImport(fixtureSnapshot(), 'take-clip'));
    expect(heard).toEqual([{ take: 'take-1', event: countdown(3) }, 'take-clip']);
    offEvents();
    offLanded();
  });
});
