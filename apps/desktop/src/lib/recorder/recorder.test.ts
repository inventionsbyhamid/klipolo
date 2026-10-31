import { describe, expect, it } from 'vitest';
import type { WorkerState } from '../../bindings/WorkerState';
import { app, device, display, recorderSetup } from '../../test/recorderBridge';
import { armOptions, defaultPicks, NO_PICKS, type Picks, whatToRecord } from './choices';
import { deviceDenied, screenStage, screenStep } from './permission';
import { phaseOf, type RecorderPhase } from './phase';

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
      mirrorCamera: true,
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
    expect(armOptions(picks({ camera: null, showCamera: true }))?.cameraBubble).toBe(false);
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
});

describe('permissions', () => {
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
