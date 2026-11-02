import { describe, expect, it } from 'vitest';
import type { WorkerState } from '../../bindings/WorkerState';
import captureCodes from '../../bindings/captureCodes.json';
import { codedMessage } from '../../bridge/call';
import { app, device, display, recorderSetup } from '../../test/recorderBridge';
import { armOptions, defaultPicks, kindPicks, NO_PICKS, type Picks, whatToRecord } from './choices';
import { deviceDenied, screenStage, screenStep } from './permission';
import { phaseOf, type RecorderPhase } from './phase';
import { RECORDER_WORDS } from './words';

const picks = (over: Partial<Picks> = {}): Picks => ({ ...NO_PICKS, display: 'main', ...over });

describe('choices', () => {
  it('default_picks_are_the_first_display_the_default_mic_and_no_camera', () => {
    const setup = recorderSetup({
      displays: [display('built-in'), display('studio')],
      mics: [device('usb'), device('built-in mic')],
      cameras: [device('facetime')],
      mic: 'built-in mic',
    });
    expect(defaultPicks(setup)).toEqual({
      kind: 'screen',
      display: 'built-in',
      appId: null,
      mic: 'built-in mic',
      camera: null,
      showCamera: true,
      mirror: true,
      cameras: { screen: null, yourself: null },
    });
  });

  it('without_a_display_nothing_can_be_recorded', () => {
    const none = defaultPicks(recorderSetup({ displays: [] }));
    expect(none.display).toBeNull();
    expect(whatToRecord(none)).toBeNull();
    expect(armOptions(none)).toBeNull();
  });

  it('the_whole_screen_records_the_chosen_display', () => {
    expect(armOptions(picks({ display: 'studio', mic: 'usb' }))).toEqual({
      what: { kind: 'screen', display: 'studio' },
      mic: 'usb',
      camera: null,
      cameraBubble: false,
      mirrorCamera: false,
    });
  });

  it('one_app_needs_an_app_and_records_it_on_the_chosen_display', () => {
    expect(armOptions(picks({ kind: 'app' }))).toBeNull();
    const [notes] = recorderSetup({ apps: [app('com.apple.Notes', 'Notes')] }).devices.apps;
    expect(whatToRecord(picks({ kind: 'app', appId: notes.appId }))).toEqual({
      kind: 'app',
      appId: 'com.apple.Notes',
      display: 'main',
    });
  });

  it('the_camera_bubble_and_mirror_go_only_with_a_camera', () => {
    expect(armOptions(picks({ camera: null, showCamera: true, mirror: true }))).toMatchObject({
      cameraBubble: false,
      mirrorCamera: false,
    });
    expect(armOptions(picks({ camera: 'facetime', showCamera: true }))).toMatchObject({
      camera: 'facetime',
      cameraBubble: true,
      mirrorCamera: true,
    });
    expect(armOptions(picks({ camera: 'facetime', showCamera: false, mirror: false }))).toMatchObject({
      cameraBubble: false,
      mirrorCamera: false,
    });
  });

  it('record_yourself_records_the_camera_alone_with_no_bubble', () => {
    expect(whatToRecord(picks({ kind: 'camera', camera: null }))).toBeNull();
    expect(armOptions(picks({ kind: 'camera', camera: null }))).toBeNull();
    // No screen, so no display is needed and none is named.
    const yourself = picks({ kind: 'camera', display: null, camera: 'facetime', mic: 'usb', showCamera: true });
    expect(armOptions(yourself)).toEqual({
      what: { kind: 'camera' },
      mic: 'usb',
      camera: 'facetime',
      cameraBubble: false,
      mirrorCamera: true,
    });
  });

  it('choosing_record_yourself_takes_the_camera_the_setup_offers', () => {
    const setup = recorderSetup({ cameras: [device('facetime'), device('desk')] });
    const camera = (over: Partial<Picks>, s = setup) => ({ ...picks(over), ...kindPicks(s, picks(over), 'camera') }).camera;
    expect(camera({})).toBe('facetime');
    expect(camera({}, { ...setup, camera: 'desk' })).toBe('desk');
    // A camera that went is never offered; with none listed there is none to take.
    expect(camera({}, { ...setup, camera: 'gone' })).toBe('facetime');
    expect(camera({}, recorderSetup({ cameras: [] }))).toBeNull();
  });

  it('each_mode_keeps_its_own_camera', () => {
    const setup = recorderSetup({ cameras: [device('facetime'), device('desk')] });
    const to = (now: Picks, kind: Picks['kind']) => ({ ...now, ...kindPicks(setup, now, kind) });
    // A screen take with no camera; Record yourself takes the camera offered, and the person picks another.
    let now = to(picks(), 'camera');
    expect(now.camera).toBe('facetime');
    now = { ...now, camera: 'desk' };
    // Back to the whole screen: no camera, as it had, so no bubble comes with it.
    now = to(now, 'screen');
    expect(now.camera).toBeNull();
    expect(armOptions(now)).toMatchObject({ camera: null, cameraBubble: false });
    // A camera chosen for the screen goes with it to one app, and comes back after Record yourself.
    now = to({ ...now, camera: 'facetime' }, 'app');
    expect(now.camera).toBe('facetime');
    now = to(now, 'camera');
    expect(now.camera).toBe('desk');
    now = to(now, 'screen');
    expect(now.camera).toBe('facetime');
  });
});

describe('permissions', () => {
  it('record_yourself_never_shows_the_screen_recording_step', () => {
    for (const screen of ['notAsked', 'denied'] as const) {
      const setup = recorderSetup({ permissions: { screen } });
      expect(screenStep(setup, picks({ kind: 'camera', camera: 'facetime' }), false)).toBe(false);
      expect(screenStep(setup, picks({ kind: 'camera', camera: 'facetime' }), true)).toBe(false);
    }
  });

  it('screen_step_shows_until_screen_recording_is_allowed', () => {
    const asked = recorderSetup({ permissions: { screen: 'notAsked' } });
    expect(screenStep(asked, picks(), false)).toBe(true);
    expect(screenStep(asked, picks({ kind: 'app' }), false)).toBe(true);
    expect(screenStep(recorderSetup({ permissions: { screen: 'denied' } }), picks(), false)).toBe(true);
    const granted = recorderSetup({ permissions: { screen: 'granted' } });
    expect(screenStep(granted, picks(), false)).toBe(false);
    // A take was refused the screen as it started: the step shows, whatever setup read.
    expect(screenStep(granted, picks(), true)).toBe(true);
  });

  it('screen_stage_asks_then_reopens_then_explains', () => {
    const fresh = recorderSetup({ permissions: { screen: 'notAsked' } });
    expect(screenStage(fresh, false)).toBe('ask');
    expect(screenStage(fresh, true)).toBe('reopen');
    const stale = recorderSetup({ permissions: { screen: 'denied' }, screenRefusedAfterReopen: true });
    expect(screenStage(stale, false)).toBe('stillRefused');
    // "Allow screen recording" again from that card: the list opened again, so Reopen is next.
    expect(screenStage(stale, true)).toBe('reopen');
  });

  it('a_denied_device_is_flagged_only_while_it_is_chosen', () => {
    const denied = recorderSetup({ permissions: { mic: 'denied', camera: 'denied' } });
    expect(deviceDenied(denied, 'mic', picks({ mic: 'usb' }))).toBe(true);
    expect(deviceDenied(denied, 'mic', picks({ mic: null }))).toBe(false);
    expect(deviceDenied(denied, 'camera', picks({ camera: null }))).toBe(false);
    expect(deviceDenied(denied, 'camera', picks({ camera: 'facetime' }))).toBe(true);
    const notAsked = recorderSetup({ permissions: { mic: 'notAsked', camera: 'granted' } });
    expect(deviceDenied(notAsked, 'mic', picks({ mic: 'usb' }))).toBe(false);
    expect(deviceDenied(notAsked, 'camera', picks({ camera: 'facetime' }))).toBe(false);
  });
});

describe('words', () => {
  it('recorder_notes_offer_only_what_klipolo_has', () => {
    const said = (code: string) => codedMessage({ code, params: {} });
    expect(said('capture.health.dropping')).toBe('Some frames are being dropped. Quitting other busy apps may help.');
    expect(said('capture.preflight.diskShort')).toBe(
      'Less than 30 minutes of space left. Free up space to record longer.',
    );
    expect(said('capture.startOver.kept')).toBe(
      "Started over. The last take couldn't be moved to the Trash, so it stays in Movies / Klipolo / Recordings.",
    );
    expect(said('capture.startOver.undoFailed')).toBe("The last take couldn't be put back. Look for it in the Trash.");
    // No quality list, no Home, no long dashes.
    for (const code of Object.keys(RECORDER_WORDS)) expect(said(code)).not.toMatch(/Light|Home|[\u2013\u2014]/);
    // The rest are the recorder's own.
    expect(said('capture.error.encoder')).toBe('The video encoder stopped.');
  });
});

describe('voice', () => {
  it('what_a_take_says_while_and_after_it_records_never_says_we', () => {
    const codes = captureCodes
      .map((entry) => entry.code)
      .filter((code) => code.startsWith('capture.health.') || code.startsWith('capture.note.'));
    expect(codes.length).toBeGreaterThan(20);
    for (const code of codes) expect(codedMessage({ code, params: {} })).not.toMatch(/\b(we|our)\b/i);
    expect(codedMessage({ code: 'capture.note.micFlapping', params: { source: 'USB Mic' } })).toBe(
      'USB Mic keeps disconnecting, so Klipolo waits until it settles before using it again.',
    );
  });
});

describe('phase', () => {
  it('phase_follows_the_setup_and_the_worker', () => {
    const setup = recorderSetup();
    const at = (worker: WorkerState | null): RecorderPhase => phaseOf({ setup, setupError: null, worker });
    expect(phaseOf({ setup: null, setupError: null, worker: null })).toBe('loading');
    expect(phaseOf({ setup: null, setupError: 'Recording is off in this test run.', worker: null })).toBe(
      'unavailable',
    );
    expect(at(null)).toBe('choosing');
    expect(at('idle')).toBe('choosing');
    expect(at('failed')).toBe('choosing');
    for (const w of ['armed', 'starting', 'countdown', 'checkingCamera'] as const) expect(at(w)).toBe('starting');
    expect(at('recording')).toBe('recording');
    expect(at('paused')).toBe('recording');
    expect(at('stopping')).toBe('saving');
    expect(at('ready')).toBe('saving');
  });
});
