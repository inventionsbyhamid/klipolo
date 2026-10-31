import { act, screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RecorderSetup } from '../../bindings/RecorderSetup';
import { useRecorder } from '../../state/recorder';
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

describe('the dialog and its choices', () => {
  it('opening_says_getting_ready_until_the_setup_is_read', async () => {
    const read = deferred<RecorderSetup>();
    fake.script.setup = () => read.promise;
    renderWithProviders(<RecordDialog />);
    let opening!: Promise<void>;
    act(() => void (opening = rec().openRecorder()));
    const dialog = screen.getByRole('dialog', { name: 'Record' });
    expect(within(dialog).getByRole('status')).toHaveTextContent('Getting ready');
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

  it('no_open_app_says_so', async () => {
    const dialog = await openOn();
    await userEvent.click(screen.getByRole('radio', { name: 'One app' }));
    expect(dialog).toHaveTextContent('No apps with windows are open.');
    expect(recordButton()).toHaveAccessibleDescription('Choose an app first');
  });

  it('no_screen_listed_keeps_record_off', async () => {
    await openOn(recorderSetup({ displays: [] }));
    expect(recordButton()).toHaveAttribute('aria-disabled', 'true');
    expect(recordButton()).toHaveAccessibleDescription('Choose a screen to record.');
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
