import { act, screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ArmedTake } from '../../bindings/ArmedTake';
import type { CaptureEvent } from '../../bindings/CaptureEvent';
import type { PresetOffer } from '../../bindings/PresetOffer';
import type { RecorderSetup } from '../../bindings/RecorderSetup';
import type { WorkerState } from '../../bindings/WorkerState';
import { REOPEN_GRACE_MS, useRecorder } from '../../state/recorder';
import { useToasts } from '../../state/toasts';
import {
  app,
  deferred,
  device,
  display,
  type FakeRecorder,
  installFakeRecorder,
  recorderSetup,
  refusal,
} from '../../test/recorderBridge';
import { renderWithProviders } from '../../test/render';
import { RecordDialog } from './RecordDialog';

const rec = () => useRecorder.getState();

// The worker's events, as the app forwards them.
const state = (s: WorkerState): CaptureEvent => ({ ev: 'state', state: s, elapsedUs: 0 });
const countdown = (leftS: number): CaptureEvent => ({ ev: 'countdown', leftS });
const gate = (...missing: string[]): CaptureEvent => ({ ev: 'gate', missing });
const meter = (source: string, peakDbfs: number): CaptureEvent => ({ ev: 'meter', source, peakDbfs });
const failed = (code: string): CaptureEvent => ({ ev: 'failed', error: { code, params: {} } });

const SUBTITLE = 'Your screen and your voice. Add your camera if you like.';
const STATUS = 'Saved in Movies / Klipolo / Recordings. Stop with Option Command R or from the menu bar.';

let fake: FakeRecorder;

beforeEach(async () => {
  fake = await installFakeRecorder();
  useToasts.setState({ toasts: [] });
});

afterEach(() => {
  vi.restoreAllMocks();
});

/** The dialog, mounted as the app mounts it, opened on `setup` once it has been read. */
async function openOn(setup: RecorderSetup | Error = recorderSetup()) {
  fake.script.setup = () => setup;
  renderWithProviders(<RecordDialog />);
  await act(() => rec().openRecorder());
  return screen.getByRole('dialog', { name: 'Record' });
}

/** Take `take`'s worker says `event`, as the app forwards it. */
function emit(take: string, event: CaptureEvent) {
  act(() => fake.emit(take, event));
}

const recordButton = () => screen.getByRole('button', { name: 'Record' });
/** The yellow buttons that can be pressed now: a disabled one is drawn grey. */
const yellows = (root: HTMLElement) =>
  [...root.querySelectorAll<HTMLElement>('.btn-primary')].filter(
    (b) => b.getAttribute('aria-disabled') !== 'true' && !b.hasAttribute('disabled'),
  );

/** Picks `choice` in the select named `name` ("Microphone: mic-1"). */
async function choose(name: string, choice: string) {
  await userEvent.click(screen.getByRole('button', { name }));
  await userEvent.click(await screen.findByRole('menuitemradio', { name: choice }));
}

/** Opens the dialog and presses Record: the take is arming, then counting down. */
async function pressRecord(setup = recorderSetup()) {
  const dialog = await openOn(setup);
  await userEvent.click(recordButton());
  return dialog;
}

describe('the dialog and its choices', () => {
  it('opening_says_getting_ready_until_the_setup_is_read', async () => {
    const read = deferred<RecorderSetup>();
    fake.script.setup = () => read.promise;
    renderWithProviders(<RecordDialog />);
    let opening!: Promise<void>;
    act(() => void (opening = rec().openRecorder()));
    const dialog = screen.getByRole('dialog', { name: 'Record' });
    expect(within(dialog).getByText('Getting ready')).toHaveAttribute('role', 'status');
    expect(screen.queryByRole('button', { name: 'Record' })).toBeNull();
    await act(async () => {
      read.resolve(recorderSetup());
      await opening;
    });
    expect(recordButton()).toBeInTheDocument();
    expect(within(dialog).queryByText('Getting ready')).toBeNull();
  });

  it('record_is_the_only_yellow_button', async () => {
    const dialog = await openOn();
    expect(dialog).toHaveTextContent(SUBTITLE);
    expect(dialog).toHaveTextContent(STATUS);
    expect(yellows(dialog)).toEqual([recordButton()]);
    expect(recordButton()).not.toHaveAttribute('aria-disabled');
    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveClass('btn-secondary');
    // Opening asked nothing and armed nothing: only the Record press does.
    expect(fake.calls).toEqual(['setup']);
    await userEvent.click(recordButton());
    expect(fake.calls).toEqual(['setup', 'arm', 'send:start']);
    expect(fake.armed[0]).toEqual({
      what: { kind: 'screen', display: 'main' },
      mic: 'mic-1',
      camera: null,
      cameraBubble: false,
      mirrorCamera: false,
    });
  });

  it('cards_are_a_radio_group_with_arrow_keys', async () => {
    await openOn(
      recorderSetup({
        displays: [
          display('main', { name: 'Built-in Retina Display' }),
          display('studio', { name: 'Studio Display', width: 5120, height: 2880 }),
          display('tv', { name: 'Living room TV', width: 3840, height: 2160 }),
        ],
      }),
    );
    expect(screen.getByRole('radiogroup', { name: 'What to record' })).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: 'Whole screen' })).toHaveAttribute('aria-checked', 'true');
    const group = screen.getByRole('radiogroup', { name: 'Screens' });
    const cards = within(group).getAllByRole('radio');
    expect(cards.map((c) => c.getAttribute('aria-checked'))).toEqual(['true', 'false', 'false']);
    expect(cards[0]).toHaveAccessibleName('Built-in Retina Display');
    expect(cards[0]).toHaveAccessibleDescription('2560 x 1440');
    expect(cards[1]).toHaveAccessibleDescription('5120 x 2880');
    // One stop in the Tab order: the checked card.
    expect(cards.map((c) => c.getAttribute('tabindex'))).toEqual(['0', '-1', '-1']);
    cards[0].focus();
    await userEvent.keyboard('{ArrowRight}');
    expect(cards[1]).toHaveFocus();
    expect(cards[1]).toHaveAttribute('aria-checked', 'true');
    expect(rec().picks.display).toBe('studio');
    await userEvent.keyboard('{ArrowDown}');
    expect(cards[2]).toHaveFocus();
    await userEvent.keyboard('{ArrowRight}');
    expect(cards[0]).toHaveFocus();
    await userEvent.keyboard('{ArrowLeft}');
    expect(cards[2]).toHaveFocus();
    expect(rec().picks.display).toBe('tv');
    await userEvent.click(cards[1]);
    expect(rec().picks.display).toBe('studio');
    expect(cards.map((c) => c.getAttribute('tabindex'))).toEqual(['-1', '0', '-1']);
  });

  it('one_app_needs_an_app_before_record', async () => {
    const dialog = await openOn(
      recorderSetup({ apps: [app('com.apple.Safari', 'Safari'), app('com.apple.Notes', 'Notes')] }),
    );
    await userEvent.click(screen.getByRole('radio', { name: 'One app' }));
    const group = screen.getByRole('radiogroup', { name: 'Apps' });
    expect(group).toHaveAccessibleDescription('Choose an app');
    expect(dialog).toHaveTextContent('Choose an app');
    expect(within(group).getAllByRole('radio').map((c) => c.getAttribute('aria-checked'))).toEqual([
      'false',
      'false',
    ]);
    // With none checked, the first card is the group's stop in the Tab order.
    expect(within(group).getByRole('radio', { name: 'Safari' })).toHaveAttribute('tabindex', '0');
    expect(recordButton()).toHaveAttribute('aria-disabled', 'true');
    expect(recordButton()).toHaveAccessibleDescription('Choose an app first');
    expect(yellows(dialog)).toEqual([]);
    await userEvent.click(recordButton());
    expect(fake.calls).toEqual(['setup']);
    await userEvent.click(within(group).getByRole('radio', { name: 'Notes' }));
    expect(dialog).not.toHaveTextContent('Choose an app');
    expect(recordButton()).not.toHaveAttribute('aria-disabled');
    await userEvent.click(recordButton());
    expect(fake.armed[0].what).toEqual({ kind: 'app', appId: 'com.apple.Notes', display: 'main' });
  });

  it('one_app_s_cards_sit_below_the_preview_in_their_own_rows', async () => {
    const dialog = await openOn(
      recorderSetup({
        apps: ['Safari', 'Notes', 'Mail', 'Keynote'].map((name) => app(`com.apple.${name}`, name)),
      }),
    );
    await userEvent.click(screen.getByRole('radio', { name: 'One app' }));
    await userEvent.click(screen.getByRole('radio', { name: 'Keynote' }));
    const preview = dialog.querySelector<HTMLElement>('.record-preview');
    const group = screen.getByRole('radiogroup', { name: 'Apps' });
    expect(within(group).getAllByRole('radio')).toHaveLength(4);
    // The cards come after the preview box, never inside it.
    expect(preview).not.toBeNull();
    expect(preview?.contains(group)).toBe(false);
    expect(preview?.compareDocumentPosition(group)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    const css = readFileSync(resolve(__dirname, 'record.css'), 'utf8');
    // The body's rows are as tall as what is in them, so a body that has to scroll never squeezes the 16:9 preview's
    // row and lets the cards' rows climb over it: it scrolls instead.
    const body = css.match(/\.record-body \{[^}]*\}/)?.[0] ?? '';
    expect(body).toMatch(/grid-auto-rows: max-content/);
    expect(body).toMatch(/overflow-y: auto/);
    // The cards wrap into rows of their own, in the flow: nothing places them over the preview.
    const cards = css.match(/\.record-cards \{[^}]*\}/)?.[0] ?? '';
    expect(cards).toMatch(/grid-template-columns: repeat\(3, minmax\(0, 1fr\)\)/);
    for (const rule of css.match(/\.record-(cards|card|section)[^{]*\{[^}]*\}/g) ?? [])
      expect(rule).not.toMatch(/position: (absolute|fixed)/);
  });

  it('no_open_app_says_so', async () => {
    const dialog = await openOn();
    await userEvent.click(screen.getByRole('radio', { name: 'One app' }));
    // The list is read as Record opens: the way to a new app is to open it again.
    expect(dialog).toHaveTextContent(
      'No apps with windows are open. Open the app you want to record, then open Record again.',
    );
    expect(recordButton()).toHaveAccessibleDescription('Choose an app first');
  });

  it('no_screen_listed_keeps_record_off', async () => {
    const dialog = await openOn(recorderSetup({ displays: [] }));
    const none = 'Klipolo found no screen to record. Close Record and open it again.';
    expect(dialog).toHaveTextContent(none);
    // There is nothing to choose: nothing says to.
    expect(dialog).not.toHaveTextContent('Choose a screen');
    expect(recordButton()).toHaveAttribute('aria-disabled', 'true');
    expect(recordButton()).toHaveAccessibleDescription(none);
    await userEvent.click(recordButton());
    expect(fake.calls).toEqual(['setup']);
  });

  it('camera_toggles_show_only_with_a_camera', async () => {
    await openOn(recorderSetup({ cameras: [device('FaceTime HD Camera')] }));
    expect(screen.queryByRole('switch')).toBeNull();
    await choose('Camera: No camera', 'FaceTime HD Camera');
    const show = screen.getByRole('switch', { name: 'Show me on screen while recording' });
    const mirror = screen.getByRole('switch', { name: 'Mirror my camera' });
    expect(show).toHaveAttribute('aria-checked', 'true');
    expect(mirror).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(mirror);
    expect(rec().picks.mirror).toBe(false);
    expect(mirror).toHaveAttribute('aria-checked', 'false');
    await choose('Camera: FaceTime HD Camera', 'No camera');
    expect(screen.queryByRole('switch')).toBeNull();
    expect(rec().picks.camera).toBeNull();
  });

  it('the_mic_list_ends_with_no_microphone', async () => {
    await openOn(recorderSetup({ mics: [device('MacBook Pro Microphone'), device('MV7')] }));
    await userEvent.click(screen.getByRole('button', { name: 'Microphone: MacBook Pro Microphone' }));
    expect((await screen.findAllByRole('menuitemradio')).map((item) => item.textContent)).toEqual([
      'MacBook Pro Microphone',
      'MV7',
      'No microphone',
    ]);
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'No microphone' }));
    expect(rec().picks.mic).toBeNull();
    expect(screen.getByRole('button', { name: 'Microphone: No microphone' })).toBeInTheDocument();
  });

  it('the_microphone_the_select_shows_is_the_one_recorded', async () => {
    // The setup's default is no longer listed (it was unplugged as it was read): the select never reads "No
    // microphone" while Record would send that one.
    await openOn(recorderSetup({ mics: [device('MacBook Pro Microphone'), device('MV7')], mic: 'unplugged' }));
    expect(screen.getByRole('button', { name: 'Microphone: MacBook Pro Microphone' })).toBeInTheDocument();
    await userEvent.click(recordButton());
    expect(fake.armed[0].mic).toBe('MacBook Pro Microphone');
  });

  it('test_run_shows_the_sentence_and_no_record', async () => {
    const dialog = await openOn(refusal('capture.error.testRun'));
    expect(within(dialog).getByRole('alert')).toHaveTextContent('Recording is off in this test run.');
    expect(screen.queryByRole('button', { name: 'Record' })).toBeNull();
    expect(yellows(dialog)).toEqual([]);
    const again = screen.getByRole('button', { name: 'Try again' });
    expect(again).toHaveClass('btn-secondary');
    fake.script.setup = () => recorderSetup();
    await userEvent.click(again);
    expect(recordButton()).toBeInTheDocument();
    expect(fake.calls).toEqual(['setup', 'setup']);
  });

  it('a_busy_recorder_says_where_to_stop_it', async () => {
    const dialog = await openOn(recorderSetup({ busy: true }));
    const busy = 'A recording is already running. Stop it from the menu bar.';
    expect(dialog).toHaveTextContent(busy);
    expect(recordButton()).toHaveAttribute('aria-disabled', 'true');
    expect(recordButton()).toHaveAccessibleDescription(busy);
    await userEvent.click(recordButton());
    expect(fake.calls).toEqual(['setup']);
  });

  it('reopening_while_the_last_take_lands_reads_the_setup_again_once_it_has_gone', async () => {
    await pressRecord();
    emit('take-1', state('recording'));
    emit('take-1', state('stopping'));
    emit('take-1', { ev: 'finished', session: 'take-1' });
    expect(rec().open).toBe(false);
    // Shift Command R before the take's worker has gone: the app still has the take.
    fake.script.setup = () => recorderSetup({ busy: true, ownTake: true });
    await act(() => rec().openRecorder());
    const dialog = screen.getByRole('dialog', { name: 'Record' });
    const adding = "Klipolo is adding your last recording. Record turns on when it's done.";
    expect(dialog).toHaveTextContent(adding);
    expect(dialog).not.toHaveTextContent('A recording is already running');
    expect(recordButton()).toHaveAttribute('aria-disabled', 'true');
    expect(recordButton()).toHaveAccessibleDescription(adding);
    // It has landed and its worker has gone: the setup is read again, and Record is on.
    fake.script.setup = () => recorderSetup();
    emit('take-1', state('idle'));
    await vi.waitFor(() => expect(recordButton()).not.toHaveAttribute('aria-disabled'));
    expect(dialog).not.toHaveTextContent(adding);
    expect(fake.calls.filter((call) => call === 'setup')).toHaveLength(3);
  });

  it('the_setup_notes_are_said_in_klipolo_s_words_and_a_blocking_one_keeps_record_off', async () => {
    const dialog = await openOn(
      recorderSetup({
        notes: [
          { code: 'capture.preflight.diskShort', params: {} },
          { code: 'capture.preflight.diskBlock', params: {} },
        ],
      }),
    );
    expect(dialog).toHaveTextContent('Less than 30 minutes of space left. Free up space to record longer.');
    const block = "There isn't enough space to record. Free up at least 2 GB.";
    expect(dialog).toHaveTextContent(block);
    expect(recordButton()).toHaveAccessibleDescription(block);
    await userEvent.click(recordButton());
    expect(fake.calls).toEqual(['setup']);
  });

  it('a_note_about_a_device_follows_the_device_chosen', async () => {
    const dialog = await openOn(
      recorderSetup({
        mics: [device('MacBook Pro Microphone'), device('Loopback', { isVirtual: true })],
        cameras: [device('FaceTime HD Camera', { inUse: true })],
      }),
    );
    expect(dialog).not.toHaveTextContent('Loopback is a virtual microphone.');
    await choose('Microphone: MacBook Pro Microphone', 'Loopback');
    expect(dialog).toHaveTextContent('Loopback is a virtual microphone. Your voice may not be recorded.');
    await choose('Camera: No camera', 'FaceTime HD Camera');
    expect(dialog).toHaveTextContent('FaceTime HD Camera is in use by another app.');
  });

  it('the_camera_s_note_at_the_take_s_quality_shows_while_a_camera_is_chosen', async () => {
    // Why, then what happens.
    const note =
      "This computer can't record your camera at the same time as the screen. Klipolo will record the screen without your camera.";
    const base = recorderSetup({ cameras: [device('FaceTime HD Camera')] });
    const offer = (over: Partial<PresetOffer>): PresetOffer => ({
      id: 'native60',
      width: 2560,
      height: 1440,
      fps: 60,
      gbPerHour: 4,
      minutesLeft: null,
      bestNative: false,
      bestGame: false,
      camera: null,
      cameraNote: null,
      ...over,
    });
    const fitted = (offers: PresetOffer[]) => ({ choices: base.choices, offers });
    const setup: RecorderSetup = {
      ...base,
      displays: [
        {
          display: 'main',
          noCamera: fitted([offer({ bestNative: true })]),
          // The take records at the best offer for a whole screen, not the first one listed.
          withCamera: fitted([
            offer({ id: 'light' }),
            offer({ bestNative: true, cameraNote: { code: 'capture.preflight.cameraWithScreen', params: {} } }),
          ]),
        },
      ],
    };
    const dialog = await openOn(setup);
    expect(dialog).not.toHaveTextContent(note);
    await choose('Camera: No camera', 'FaceTime HD Camera');
    expect(dialog).toHaveTextContent(note);
    await choose('Camera: FaceTime HD Camera', 'No camera');
    expect(dialog).not.toHaveTextContent(note);
  });

  it('a_mic_klipolo_chose_instead_says_so_in_klipolo_s_voice', async () => {
    const dialog = await openOn(
      recorderSetup({
        mics: [device('MacBook Pro Microphone')],
        notes: [
          { code: 'capture.preflight.micMissing', params: { fallback: 'MacBook Pro Microphone' } },
          { code: 'capture.preflight.bluetoothMic', params: { fallback: 'MacBook Pro Microphone' } },
        ],
      }),
    );
    expect(dialog).toHaveTextContent(
      "The microphone you chose isn't connected, so Klipolo picked MacBook Pro Microphone.",
    );
    expect(dialog).toHaveTextContent(
      "Your headphones' microphone lowers the sound quality while recording, so Klipolo picked MacBook Pro Microphone.",
    );
    expect(dialog.textContent).not.toMatch(/\bwe\b/i);
  });

  it('focus_is_trapped_and_returns_to_the_trigger', async () => {
    fake.script.setup = () => recorderSetup();
    renderWithProviders(
      <>
        <button type="button">Record your screen</button>
        <RecordDialog />
      </>,
    );
    const trigger = screen.getByRole('button', { name: 'Record your screen' });
    trigger.focus();
    await act(() => rec().openRecorder());
    const dialog = screen.getByRole('dialog', { name: 'Record' });
    // The title has the keyboard first, never Record: an Enter meant for something else can't record.
    expect(screen.getByRole('heading', { name: 'Record' })).toHaveFocus();
    const close = screen.getByRole('button', { name: 'Close' });
    await userEvent.tab();
    expect(close).toHaveFocus();
    await userEvent.tab({ shift: true });
    expect(recordButton()).toHaveFocus();
    await userEvent.tab();
    expect(close).toHaveFocus();
    // Focus that leaves the dialog comes back into it.
    act(() => trigger.focus());
    expect(dialog.contains(document.activeElement)).toBe(true);
    // Keys typed in it never reach the editor behind it.
    const behind = vi.fn();
    window.addEventListener('keydown', behind);
    await userEvent.keyboard('k');
    window.removeEventListener('keydown', behind);
    expect(behind).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(rec().open).toBe(false);
    expect(trigger).toHaveFocus();
  });

  it('a_select_s_menu_keeps_the_keyboard_and_esc_closes_only_the_menu', async () => {
    await openOn(recorderSetup({ mics: [device('MacBook Pro Microphone'), device('MV7')] }));
    await userEvent.click(screen.getByRole('button', { name: 'Microphone: MacBook Pro Microphone' }));
    const item = await screen.findByRole('menuitemradio', { name: 'MV7' });
    expect(item.closest('[role="menu"]')?.contains(document.activeElement)).toBe(true);
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('menu')).toBeNull();
    expect(rec().open).toBe(true);
    expect(screen.getByRole('dialog', { name: 'Record' })).toBeInTheDocument();
  });

  it('no_red_and_no_em_or_en_dash', async () => {
    const css = readFileSync(resolve(__dirname, 'record.css'), 'utf8');
    expect(css).not.toMatch(/--color-cut/);
    // Tokens only: no raw colours.
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(/i);
    const dialog = await openOn(
      recorderSetup({
        apps: [app('com.apple.Safari', 'Safari')],
        cameras: [device('FaceTime HD Camera')],
        permissions: { mic: 'denied', screen: 'notAsked' },
        screenRefusedAfterReopen: true,
        notes: [{ code: 'capture.preflight.lowBattery', params: {} }],
      }),
    );
    const said = [dialog.textContent ?? ''];
    await choose('Camera: No camera', 'FaceTime HD Camera');
    await userEvent.click(screen.getByRole('radio', { name: 'One app' }));
    said.push(dialog.textContent ?? '');
    // An en dash or an em dash, named by code so this file has neither.
    const dashes = new RegExp(`[${String.fromCharCode(0x2013, 0x2014)}]`);
    for (const text of said) expect(text).not.toMatch(dashes);
  });
});

describe('permission steps', () => {
  it('permission_step_replaces_record_until_allowed', async () => {
    const dialog = await openOn(recorderSetup({ permissions: { screen: 'notAsked' } }));
    // Ask: the step's button is the one yellow, and Record waits for it.
    expect(dialog).toHaveTextContent('Klipolo needs your permission to record your screen.');
    const step = screen.getByRole('button', { name: 'Allow screen recording' });
    expect(yellows(dialog)).toEqual([step]);
    expect(recordButton()).toHaveAttribute('aria-disabled', 'true');
    expect(recordButton()).toHaveAccessibleDescription('Allow screen recording first');
    expect(screen.queryByRole('button', { name: 'Open privacy settings' })).toBeNull();
    // A press on Record goes to the step's button: macOS's own prompt is never the only explanation.
    await userEvent.click(recordButton());
    expect(step).toHaveFocus();
    expect(fake.calls).toEqual(['setup']);
    await userEvent.click(step);
    expect(fake.calls).toEqual(['setup', 'allowScreen']);
    // Reopen: the same button, so the keyboard stays on it.
    expect(dialog).toHaveTextContent('Turn on Klipolo in the list that opened, then reopen Klipolo.');
    expect(step).toHaveAccessibleName('Reopen Klipolo');
    expect(step).toHaveFocus();
    expect(yellows(dialog)).toEqual([step]);
    const settings = screen.getByRole('button', { name: 'Open privacy settings' });
    expect(settings).toHaveClass('btn-secondary');
    await userEvent.click(settings);
    expect(fake.calls).toEqual(['setup', 'allowScreen', 'openPrivacy:screen']);
    // A double click on Allow never reopens Klipolo; a press after that does.
    vi.spyOn(Date, 'now').mockReturnValue(Date.now() + REOPEN_GRACE_MS * 10);
    await userEvent.click(step);
    expect(fake.calls).toEqual(['setup', 'allowScreen', 'openPrivacy:screen', 'reopen']);
    expect(recordButton()).toHaveAttribute('aria-disabled', 'true');
  });

  it('a_screen_still_refused_after_reopen_says_how_to_remove_the_older_copy', async () => {
    const dialog = await openOn(
      recorderSetup({ permissions: { screen: 'denied' }, screenRefusedAfterReopen: true }),
    );
    expect(dialog).toHaveTextContent(
      "macOS still isn't letting Klipolo record the screen. This can happen when an older copy of Klipolo is in the list.",
    );
    expect(dialog).toHaveTextContent(
      'Open Screen & System Audio Recording, select Klipolo, remove it with the minus button, then press Allow again.',
    );
    const settings = screen.getByRole('button', { name: 'Open privacy settings' });
    const allow = screen.getByRole('button', { name: 'Allow screen recording' });
    expect(settings).toHaveClass('btn-secondary');
    expect(yellows(dialog)).toEqual([allow]);
    // The primary is last.
    expect(settings.compareDocumentPosition(allow) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(recordButton()).toHaveAccessibleDescription('Allow screen recording first');
  });

  it('reopen_from_a_disk_image_says_to_move_klipolo_first', async () => {
    // Run from its disk image, or from the copy macOS made of it where it was downloaded, Klipolo couldn't open again.
    const dialog = await openOn(recorderSetup({ permissions: { screen: 'notAsked' } }));
    fake.script.reopen = () => refusal('capture.permission.reopenMoveFirst');
    await userEvent.click(screen.getByRole('button', { name: 'Allow screen recording' }));
    vi.spyOn(Date, 'now').mockReturnValue(Date.now() + REOPEN_GRACE_MS * 10);
    await userEvent.click(screen.getByRole('button', { name: 'Reopen Klipolo' }));
    expect(within(dialog).getByRole('alert')).toHaveTextContent(
      'Move Klipolo to your Applications folder, then open it again.',
    );
    expect(fake.calls).toEqual(['setup', 'allowScreen', 'reopen']);
  });

  it('a_refused_permission_press_is_said_in_the_step', async () => {
    const dialog = await openOn(recorderSetup({ permissions: { screen: 'notAsked' } }));
    fake.script.allowScreen = () => refusal('capture.busy', { place: { code: 'capture.place.menuBar', params: {} } });
    await userEvent.click(screen.getByRole('button', { name: 'Allow screen recording' }));
    expect(within(dialog).getByRole('alert')).toHaveTextContent(/^A recording is already running\./);
    // Nothing offers to record again from a refused permission press.
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('denied_mic_shows_the_sentence_and_open_privacy_settings', async () => {
    const dialog = await openOn(recorderSetup({ permissions: { mic: 'denied' } }));
    const sentence = "Klipolo can't use your microphone. Allow it in Privacy & Security, then try again.";
    expect(dialog).toHaveTextContent(sentence);
    const settings = screen.getByRole('button', { name: 'Open privacy settings' });
    expect(settings).toHaveClass('btn-ghost');
    expect(settings).toHaveAccessibleDescription(sentence);
    await userEvent.click(settings);
    expect(fake.calls).toEqual(['setup', 'openPrivacy:mic']);
    // "No microphone" still records: the sentence goes with the mic.
    await choose('Microphone: mic-1', 'No microphone');
    expect(dialog).not.toHaveTextContent(sentence);
    expect(recordButton()).not.toHaveAttribute('aria-disabled');
  });

  it('denied_camera_shows_the_sentence_once_a_camera_is_chosen', async () => {
    const dialog = await openOn(
      recorderSetup({ cameras: [device('FaceTime HD Camera')], permissions: { camera: 'denied' } }),
    );
    const sentence = "Klipolo can't use your camera. Allow it in Privacy & Security, then try again.";
    expect(dialog).not.toHaveTextContent(sentence);
    await choose('Camera: No camera', 'FaceTime HD Camera');
    expect(dialog).toHaveTextContent(sentence);
    await userEvent.click(screen.getByRole('button', { name: 'Open privacy settings' }));
    expect(fake.calls).toEqual(['setup', 'openPrivacy:camera']);
  });

  it('a_denied_camera_with_no_camera_chosen_says_nothing_about_the_camera', async () => {
    // macOS refused the camera once; every opening starts on "No camera", which records all the same.
    const dialog = await openOn(
      recorderSetup({ cameras: [device('FaceTime HD Camera')], permissions: { camera: 'denied' } }),
    );
    expect(dialog).not.toHaveTextContent(/camera.*then try again/);
    expect(dialog.querySelector('.record-notes')).toBeNull();
    expect(recordButton()).not.toHaveAttribute('aria-disabled');
    // Chosen, it is said once, under its row.
    await choose('Camera: No camera', 'FaceTime HD Camera');
    expect(dialog.textContent!.match(/use your camera/g)).toHaveLength(1);
  });

  it('a_denied_mic_is_said_once_under_its_row', async () => {
    const dialog = await openOn(recorderSetup({ permissions: { mic: 'denied' } }));
    expect(dialog.textContent!.match(/use your microphone/g)).toHaveLength(1);
    expect(dialog.querySelector('.record-notes')).toBeNull();
    // With "No microphone", nothing asks for it.
    await choose('Microphone: mic-1', 'No microphone');
    expect(dialog).not.toHaveTextContent(/microphone.*then try again/);
    expect(dialog.querySelector('.record-notes')).toBeNull();
  });

  it('a_refused_open_privacy_settings_is_said_without_try_again', async () => {
    const dialog = await openOn(recorderSetup({ permissions: { mic: 'denied' } }));
    fake.script.openPrivacy = () => refusal('capture.error.os');
    await userEvent.click(screen.getByRole('button', { name: 'Open privacy settings' }));
    expect(within(dialog).getByRole('alert')).toHaveTextContent('Recording failed.');
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('a_mic_not_asked_yet_offers_allow_microphone_in_place_of_the_hint', async () => {
    const hint = 'macOS asks to use your microphone when you press Record.';
    const dialog = await openOn(recorderSetup({ permissions: { mic: 'notAsked' } }));
    expect(dialog).not.toHaveTextContent(hint);
    expect(screen.getByRole('button', { name: 'Allow microphone' })).toHaveClass('btn-secondary');
    await choose('Microphone: mic-1', 'No microphone');
    expect(screen.queryByRole('button', { name: 'Allow microphone' })).toBeNull();
  });

  it('a_take_refused_the_microphone_leaves_neither_the_hint_nor_allow', async () => {
    const dialog = await pressRecord(recorderSetup({ permissions: { mic: 'notAsked' } }));
    emit('take-1', failed('capture.permission.mic'));
    // macOS remembers the no: neither would come true. The preview's note says what to do, with its page.
    expect(dialog).not.toHaveTextContent('macOS asks to use your microphone when you press Record.');
    expect(screen.queryByRole('button', { name: 'Allow microphone' })).toBeNull();
    expect(screen.getAllByRole('button', { name: 'Open privacy settings' })).toHaveLength(1);
  });

  it('without_privacy_pages_there_is_no_open_privacy_settings', async () => {
    const dialog = await openOn(recorderSetup({ permissions: { mic: 'denied' }, privacySettings: false }));
    expect(dialog).toHaveTextContent("Klipolo can't use your microphone.");
    expect(screen.queryByRole('button', { name: 'Open privacy settings' })).toBeNull();
  });
});

describe('countdown, saving and failure', () => {
  it('countdown_shows_the_number_meter_and_cancel', async () => {
    const dialog = await pressRecord();
    // Arming, before the worker counts: Getting ready, and Cancel already.
    expect(dialog).toHaveTextContent('Getting ready');
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    expect(cancel).toHaveFocus();
    expect(screen.queryByRole('button', { name: 'Record' })).toBeNull();
    emit('take-1', countdown(3));
    expect(dialog.querySelector('.record-count')).toHaveTextContent('3');
    expect(dialog).toHaveTextContent('Recording starts in 3 s.');
    const announced = within(dialog)
      .getAllByRole('status')
      .find((s) => s.textContent === 'Recording starts in 3 s.');
    expect(announced).toBeDefined();
    emit('take-1', countdown(2));
    expect(dialog.querySelector('.record-count')).toHaveTextContent('2');
    expect(dialog).toHaveTextContent('Recording starts in 2 s.');
    // Announced once: the status keeps its first words.
    expect(announced).toHaveTextContent('Recording starts in 3 s.');
    emit('take-1', meter('mic', -6));
    const level = screen.getByRole('meter', { name: 'Microphone level' });
    expect(level).toHaveAttribute('aria-valuenow', '90');
    emit('take-1', meter('mic', -80));
    expect(level).toHaveAttribute('aria-valuenow', '0');
    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveClass('btn-secondary');
    expect(yellows(dialog)).toEqual([]);
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(fake.calls).toEqual(['setup', 'arm', 'send:start', 'send:cancel']);
    expect(recordButton()).toBeInTheDocument();
  });

  it('no_meter_without_a_microphone', async () => {
    await openOn();
    await choose('Microphone: mic-1', 'No microphone');
    await userEvent.click(recordButton());
    emit('take-1', countdown(3));
    expect(screen.queryByRole('meter')).toBeNull();
  });

  it('esc_closes_while_choosing_and_cancels_the_countdown', async () => {
    await pressRecord();
    emit('take-1', countdown(3));
    await userEvent.keyboard('{Escape}');
    expect(fake.calls).toContain('send:cancel');
    expect(rec().open).toBe(true);
    expect(recordButton()).toBeInTheDocument();
    // After a cancel the keyboard is on the title, never on Record: a second Enter can't record.
    expect(screen.getByRole('heading', { name: 'Record' })).toHaveFocus();
    await userEvent.keyboard('{Escape}');
    expect(rec().open).toBe(false);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('the_close_button_cancels_the_countdown_and_closes', async () => {
    await pressRecord();
    emit('take-1', countdown(3));
    await userEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(fake.calls).toContain('send:cancel');
    expect(rec().open).toBe(false);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('the_gate_asks_about_each_silent_source', async () => {
    const dialog = await pressRecord(recorderSetup({ cameras: [device('FaceTime HD Camera')] }));
    emit('take-1', countdown(3));
    emit('take-1', gate('camera', 'mic'));
    expect(dialog).toHaveTextContent("Camera isn't sending anything yet.");
    expect(dialog).toHaveTextContent("Mic isn't sending anything yet.");
    expect(dialog).not.toHaveTextContent('Recording starts in');
    const without = screen.getAllByRole('button', { name: 'Record without it' });
    expect(without).toHaveLength(2);
    expect(without[0]).toHaveClass('btn-secondary');
    expect(without[0]).toHaveAccessibleDescription("Camera isn't sending anything yet.");
    expect(without[0]).toHaveFocus();
    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveClass('btn-ghost');
    await userEvent.click(without[0]);
    expect(fake.sent.at(-1)).toEqual({ take: 'take-1', command: { cmd: 'withoutSource', source: 'camera' } });
    expect(dialog).not.toHaveTextContent("Camera isn't sending anything yet.");
    expect(dialog).toHaveTextContent("Mic isn't sending anything yet.");
  });

  it('the_dialog_hides_while_recording_and_saving', async () => {
    await pressRecord();
    emit('take-1', countdown(1));
    emit('take-1', state('recording'));
    expect(rec().open).toBe(true);
    expect(screen.queryByRole('dialog')).toBeNull();
    emit('take-1', state('stopping'));
    expect(screen.queryByRole('dialog')).toBeNull();
    // Saved: the take lands and the dialog goes for good.
    emit('take-1', { ev: 'finished', session: 'take-1' });
    expect(rec().open).toBe(false);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('a_take_that_fails_says_why_and_try_again_records_again', async () => {
    const dialog = await openOn();
    fake.script.arm = () => refusal('capture.error.busy');
    await userEvent.click(recordButton());
    const alert = within(dialog).getByRole('alert');
    expect(alert).toHaveTextContent('That device is in use.');
    expect(alert.querySelector('.ico')).not.toBeNull();
    expect(screen.getByRole('heading', { name: 'Record' })).toHaveFocus();
    const again = screen.getByRole('button', { name: 'Try again' });
    expect(again).toHaveClass('btn-secondary');
    expect(yellows(dialog)).toEqual([recordButton()]);
    fake.script.arm = undefined;
    await userEvent.click(again);
    expect(fake.calls).toEqual(['setup', 'arm', 'arm', 'send:start']);
    expect(within(dialog).queryByRole('alert')).toBeNull();
  });

  it('a_take_refused_the_microphone_offers_its_privacy_settings', async () => {
    const dialog = await pressRecord();
    emit('take-1', failed('capture.permission.mic'));
    expect(within(dialog).getByRole('alert')).toHaveTextContent(
      "Allow Klipolo to use your microphone in your computer's privacy settings, then try again.",
    );
    await userEvent.click(screen.getByRole('button', { name: 'Open privacy settings' }));
    expect(fake.calls).toContain('openPrivacy:mic');
  });

  it('a_take_refused_the_screen_shows_the_step_with_why', async () => {
    const dialog = await pressRecord();
    emit('take-1', failed('capture.permission.screen'));
    expect(dialog).toHaveTextContent('Klipolo needs your permission to record your screen.');
    expect(within(dialog).getByRole('alert')).toHaveTextContent(
      'Allow Klipolo to record your screen, then reopen Klipolo.',
    );
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(recordButton()).toHaveAttribute('aria-disabled', 'true');
  });

  it('a_refused_record_without_it_is_said_in_the_countdown', async () => {
    const dialog = await pressRecord();
    emit('take-1', countdown(3));
    emit('take-1', gate('mic'));
    fake.script.send = (_take, command) =>
      command.cmd === 'withoutSource' ? refusal('capture.error.os') : undefined;
    await userEvent.click(screen.getByRole('button', { name: 'Record without it' }));
    expect(within(dialog).getByRole('alert')).toHaveTextContent('Recording failed.');
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('a_refused_privacy_press_after_a_failed_take_offers_no_try_again', async () => {
    const dialog = await openOn(recorderSetup({ permissions: { mic: 'denied' } }));
    fake.script.arm = () => refusal('capture.error.os');
    await userEvent.click(recordButton());
    expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument();
    // The same words again, from the microphone's own "Open privacy settings": that press's, not a take's.
    fake.script.openPrivacy = () => refusal('capture.error.os');
    await userEvent.click(screen.getByRole('button', { name: 'Open privacy settings' }));
    expect(within(dialog).getByRole('alert')).toHaveTextContent('Recording failed.');
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('cancel_after_a_refused_record_without_it_brings_back_the_choices_with_nothing_to_try_again', async () => {
    const dialog = await pressRecord();
    emit('take-1', countdown(3));
    emit('take-1', gate('mic'));
    fake.script.send = (_take, command) =>
      command.cmd === 'withoutSource' ? refusal('capture.error.os') : undefined;
    await userEvent.click(screen.getByRole('button', { name: 'Record without it' }));
    expect(within(dialog).getByRole('alert')).toHaveTextContent('Recording failed.');
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(recordButton()).toBeInTheDocument();
    expect(within(dialog).queryByRole('alert')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('a_take_refused_the_camera_offers_its_page_by_the_code_not_the_words', async () => {
    await openOn(recorderSetup({ cameras: [device('FaceTime HD Camera')] }));
    // Worded differently than the code's own sentence (a value, another language): the code still says which page.
    fake.script.arm = () =>
      new Error('Klipolo may not use the camera.', { cause: { code: 'capture.permission.camera', params: {} } });
    await userEvent.click(recordButton());
    await userEvent.click(screen.getByRole('button', { name: 'Open privacy settings' }));
    expect(fake.calls).toContain('openPrivacy:camera');
  });

  it('try_again_with_one_app_and_none_chosen_asks_for_an_app', async () => {
    const dialog = await openOn(recorderSetup({ apps: [app('com.apple.Safari', 'Safari')] }));
    fake.script.arm = () => refusal('capture.error.busy');
    await userEvent.click(recordButton());
    await userEvent.click(screen.getByRole('radio', { name: 'One app' }));
    const again = screen.getByRole('button', { name: 'Try again' });
    expect(again).toHaveAttribute('aria-disabled', 'true');
    expect(again).toHaveAccessibleDescription('Choose an app first');
    await userEvent.click(again);
    // It arms nothing, and the keyboard goes to the apps to pick one.
    expect(fake.calls).toEqual(['setup', 'arm']);
    expect(screen.getByRole('radio', { name: 'Safari' })).toHaveFocus();
    expect(dialog).toHaveTextContent('Choose an app');
    await userEvent.click(screen.getByRole('radio', { name: 'Safari' }));
    expect(again).not.toHaveAttribute('aria-disabled');
    fake.script.arm = undefined;
    await userEvent.click(again);
    expect(fake.armed.at(-1)?.what).toEqual({ kind: 'app', appId: 'com.apple.Safari', display: 'main' });
  });

  it('record_is_one_element_as_it_turns_off_and_on', async () => {
    await openOn(recorderSetup({ apps: [app('com.apple.Safari', 'Safari')] }));
    const record = recordButton();
    record.focus();
    await userEvent.click(screen.getByRole('radio', { name: 'One app' }));
    expect(recordButton()).toBe(record);
    expect(record).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(screen.getByRole('radio', { name: 'Safari' }));
    expect(recordButton()).toBe(record);
    expect(record).not.toHaveAttribute('aria-disabled');
    expect(record).not.toHaveAccessibleDescription('Choose an app first');
  });

  it('arming_that_waits_still_says_getting_ready_and_cancels', async () => {
    await openOn();
    const arming = deferred<ArmedTake>();
    fake.script.arm = () => arming.promise;
    await userEvent.click(recordButton());
    expect(screen.getByRole('dialog', { name: 'Record' })).toHaveTextContent('Getting ready');
    await userEvent.keyboard('{Escape}');
    expect(recordButton()).toBeInTheDocument();
    await act(async () => arming.resolve({ id: 'take-1', countdownS: 3 }));
    // Cancelled while arming: its worker goes at once and never starts.
    expect(fake.calls).toEqual(['setup', 'arm', 'send:disarm']);
  });
});
