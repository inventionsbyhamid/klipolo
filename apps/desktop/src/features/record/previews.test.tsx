import { act, screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CaptureEvent } from '../../bindings/CaptureEvent';
import type { PreviewId } from '../../bindings/PreviewId';
import type { Previews } from '../../bindings/Previews';
import type { RecorderSetup } from '../../bindings/RecorderSetup';
import { useRecorder } from '../../state/recorder';
import { useRecorderPreviews } from '../../state/recorderPreviews';
import {
  app,
  device,
  display,
  type FakeRecorder,
  installFakeRecorder,
  recorderSetup,
  refusal,
} from '../../test/recorderBridge';
import { frameIn, nameAt, sendFrame, shownIn } from '../../test/recorderPreviews';
import { renderWithProviders } from '../../test/render';
import { STALE_MS } from './LivePreview';
import { RecordDialog } from './RecordDialog';

const rec = () => useRecorder.getState();
const previews = () => useRecorderPreviews.getState();

let fake: FakeRecorder;

beforeEach(async () => {
  fake = await installFakeRecorder();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  // Back to the page's own visibility (`visible`).
  delete (document as { visibilityState?: unknown }).visibilityState;
});

/** Lets the previews' calls on their way reach the fake app (microtasks only: fine under fake timers too). */
const flush = () =>
  act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });

/** The dialog, mounted as the app mounts it, opened on `setup` once it has been read, with its previews asked for. */
async function openOn(setup: RecorderSetup | Error = recorderSetup()) {
  fake.script.setup = () => setup;
  renderWithProviders(<RecordDialog />);
  await act(() => rec().openRecorder());
  await flush();
  return screen.getByRole('dialog', { name: 'Record' });
}

/** What the dialog wants previewed for the default setup's choices, as `over` changes it. */
const want = (over: Partial<Previews> = {}): Previews => ({
  thumbs: true,
  live: { kind: 'screen', display: 'main' },
  camera: null,
  mic: 'mic-1',
  ask: null,
  ...over,
});

/** A preview of the app's says `event`. */
function emit(id: PreviewId | string, event: CaptureEvent) {
  act(() => fake.emit(id, event));
}
const failed = (code: string): CaptureEvent => ({ ev: 'failed', error: { code, params: {} } });

/** Picks `choice` in the select named `name` ("Microphone: mic-1"). */
async function choose(name: string, choice: string) {
  await userEvent.click(screen.getByRole('button', { name }));
  await userEvent.click(await screen.findByRole('menuitemradio', { name: choice }));
  await flush();
}

const card = (name: string) => screen.getByRole('radio', { name });
const live = () => screen.queryByRole('img', { name: /^Live view of / });
const recordButton = () => screen.getByRole('button', { name: 'Record' });
const calls = () => fake.previewed.filter((w) => w !== 'off');

/** Klipolo's window goes behind, or comes back in front. */
async function windowFocus(event: 'blur' | 'focus') {
  act(() => void window.dispatchEvent(new Event(event)));
  await flush();
}

/** The window is hidden (minimised) or shown again. */
async function visibility(state: DocumentVisibilityState) {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state });
  act(() => void document.dispatchEvent(new Event('visibilitychange')));
  await flush();
}

describe('the previews in the Record dialog', () => {
  it('cards_show_thumbnails_as_they_arrive', async () => {
    await openOn(
      recorderSetup({
        displays: [display('main', { name: 'Built-in Retina Display' }), display('studio', { name: 'Studio Display' })],
        apps: [app('com.apple.Safari', 'Safari')],
      }),
    );
    expect(fake.previewed).toEqual([want()]);
    // Until a still arrives, a card shows its icon.
    expect(shownIn(card('Built-in Retina Display'))).toBeNull();
    expect(card('Built-in Retina Display').querySelector('.ico')).not.toBeNull();
    await sendFrame(fake, 'thumb', 'display:main', 'main 1');
    expect(await frameIn(card('Built-in Retina Display'))).toBe('main 1');
    expect(shownIn(card('Studio Display'))).toBeNull();
    await sendFrame(fake, 'thumb', 'display:studio', 'studio 1');
    await sendFrame(fake, 'thumb', 'display:main', 'main 2');
    expect(await frameIn(card('Built-in Retina Display'))).toBe('main 2');
    expect(await frameIn(card('Studio Display'))).toBe('studio 1');
    // An app's stills are on its card.
    await sendFrame(fake, 'thumb', 'app:com.apple.Safari', 'safari 1');
    await userEvent.click(screen.getByRole('radio', { name: 'One app' }));
    expect(await frameIn(card('Safari'))).toBe('safari 1');
  });

  it('a_card_whose_still_stops_coming_turns_grey', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] });
    await openOn();
    const pic = () => card('Display main').querySelector('.record-pic');
    await sendFrame(fake, 'thumb', 'display:main', 'main 1');
    await sendFrame(fake, 'live', 'display:main', 'live 1');
    expect(pic()).not.toHaveAttribute('data-stale');
    expect(live()).not.toHaveAttribute('data-stale');
    act(() => void vi.advanceTimersByTime(STALE_MS - 1));
    expect(pic()).not.toHaveAttribute('data-stale');
    // A few seconds without a new one: a frozen picture never looks live.
    act(() => void vi.advanceTimersByTime(1));
    expect(pic()).toHaveAttribute('data-stale');
    expect(live()).toHaveAttribute('data-stale');
    await sendFrame(fake, 'thumb', 'display:main', 'main 2');
    expect(pic()).not.toHaveAttribute('data-stale');
    // One that was already old when the app got it is grey at once.
    await sendFrame(fake, 'thumb', 'display:main', 'main 3', Date.now() - STALE_MS);
    expect(pic()).toHaveAttribute('data-stale');
  });

  it('live_preview_follows_the_chosen_card', async () => {
    await openOn(
      recorderSetup({ displays: [display('main'), display('studio')], apps: [app('com.apple.Safari', 'Safari')] }),
    );
    expect(live()).toBeNull();
    await sendFrame(fake, 'live', 'display:main', 'live main 1');
    expect(live()).toHaveAccessibleName('Live view of Display main');
    expect(live()?.closest('.record-preview')).not.toBeNull();
    expect(await frameIn(live())).toBe('live main 1');

    await userEvent.click(card('Display studio'));
    await flush();
    expect(fake.previewed.at(-1)).toEqual(want({ live: { kind: 'screen', display: 'studio' } }));
    // The last choice's picture goes at once, and one of it still on its way is never shown.
    expect(live()).toBeNull();
    await sendFrame(fake, 'live', 'display:main', 'live main 2');
    expect(live()).toBeNull();
    await sendFrame(fake, 'live', 'display:studio', 'live studio 1');
    expect(live()).toHaveAccessibleName('Live view of Display studio');
    expect(await frameIn(live())).toBe('live studio 1');

    // One app: nothing live until an app is chosen, then its windows on the screen chosen.
    await userEvent.click(screen.getByRole('radio', { name: 'One app' }));
    await flush();
    expect(fake.previewed.at(-1)).toEqual(want({ live: null }));
    expect(live()).toBeNull();
    await userEvent.click(card('Safari'));
    await flush();
    expect(fake.previewed.at(-1)).toEqual(
      want({ live: { kind: 'app', appId: 'com.apple.Safari', display: 'studio' } }),
    );
    await sendFrame(fake, 'live', 'app:com.apple.Safari', 'live safari');
    expect(live()).toHaveAccessibleName('Live view of Safari');
    expect(await frameIn(live())).toBe('live safari');
  });



  it('previews_stop_on_close_record_and_blur', async () => {
    await openOn();
    expect(fake.previewed).toEqual([want()]);
    await sendFrame(fake, 'thumb', 'display:main', 'main 1');
    // Klipolo goes behind: every preview stops, and its pictures go with it.
    await windowFocus('blur');
    expect(fake.previewed.at(-1)).toBe('off');
    expect(shownIn(card('Display main'))).toBeNull();
    // A frame still on its way is never shown.
    await sendFrame(fake, 'thumb', 'display:main', 'main 2');
    expect(shownIn(card('Display main'))).toBeNull();
    // In front again: the app started nothing by itself, so the dialog asks again.
    await windowFocus('focus');
    expect(fake.previewed.at(-1)).toEqual(want());
    // Hidden, then shown.
    await visibility('hidden');
    expect(fake.previewed.at(-1)).toBe('off');
    await visibility('visible');
    expect(fake.previewed.at(-1)).toEqual(want());
    // Record: they stop before the take arms, and nothing starts during the countdown.
    await userEvent.click(recordButton());
    await flush();
    expect(fake.log.slice(-3)).toEqual(['previewsOff', 'arm', 'send:start']);
    // Cancel brings back the choices, and the previews with them.
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await flush();
    expect(fake.log.slice(-2)).toEqual(['send:cancel', 'previews']);
    // Try again stops them too.
    fake.script.arm = () => refusal('capture.error.busy');
    await userEvent.click(recordButton());
    await flush();
    fake.script.arm = undefined;
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await flush();
    expect(fake.log.slice(-3)).toEqual(['previewsOff', 'arm', 'send:start']);
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await flush();
    // Closing stops them.
    await sendFrame(fake, 'thumb', 'display:main', 'main 3');
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await flush();
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(fake.previewed.at(-1)).toBe('off');
    expect(previews().thumbs).toEqual({});
  });




  it('blob_urls_are_revoked', async () => {
    await openOn(recorderSetup({ cameras: [device('cam')] }));
    await choose('Camera: No camera', 'cam');
    const urls: string[] = [];
    for (const [kind, key, url] of [
      ['thumb', 'display:main', () => previews().thumbs['display:main']],
      ['live', 'display:main', () => previews().live],
      ['camera', 'cam', () => previews().camera],
    ] as const) {
      await sendFrame(fake, kind, key, `${kind} 1`);
      const first = url() ?? '';
      expect(await nameAt(first)).toBe(`${kind} 1`);
      await sendFrame(fake, kind, key, `${kind} 2`);
      const second = url() ?? '';
      // Replaced: the last one is let go.
      expect(await nameAt(first)).toBeNull();
      expect(await nameAt(second)).toBe(`${kind} 2`);
      urls.push(second);
    }
    // A stop lets every picture go.
    await windowFocus('blur');
    expect(previews()).toMatchObject({ thumbs: {}, live: null, camera: null, meterDbfs: null });
    for (const url of urls) expect(await nameAt(url)).toBeNull();
    // So does closing.
    await windowFocus('focus');
    await sendFrame(fake, 'thumb', 'display:main', 'thumb 3');
    const last = previews().thumbs['display:main'];
    act(() => rec().close());
    await flush();
    expect(await nameAt(last)).toBeNull();
    expect(previews().thumbs).toEqual({});
  });

  it('each_drawn_frame_is_acked_once', async () => {
    await openOn(recorderSetup({ cameras: [device('cam')] }));
    await choose('Camera: No camera', 'cam');
    await sendFrame(fake, 'thumb', 'display:main', 'thumb 1');
    await sendFrame(fake, 'live', 'display:main', 'live 1');
    await sendFrame(fake, 'camera', 'cam', 'camera 1');
    await sendFrame(fake, 'thumb', 'display:main', 'thumb 2');
    expect(fake.acks).toEqual(['thumb', 'live', 'camera', 'thumb']);
    // A frame that isn't drawn (of an earlier choice) is acked too, so the app never waits on it.
    await sendFrame(fake, 'live', 'display:elsewhere', 'live 2');
    expect(await nameAt(previews().live)).toBe('live 1');
    expect(fake.acks).toEqual(['thumb', 'live', 'camera', 'thumb', 'live']);
  });

  it('nothing_starts_in_a_test_run', async () => {
    await openOn(refusal('capture.error.testRun'));
    expect(screen.getByRole('alert')).toHaveTextContent('Recording is off in this test run.');
    expect(fake.previewed).toEqual([]);
    expect(fake.log).toEqual(['setup']);
  });

  it('an_app_in_a_test_run_answering_the_previews_shows_nothing', async () => {
    fake.script.previews = () => refusal('capture.error.testRun');
    const dialog = await openOn();
    expect(fake.previewed).toEqual([want()]);
    expect(within(dialog).queryByRole('alert')).toBeNull();
    expect(live()).toBeNull();
    expect(screen.queryByRole('meter')).toBeNull();
    await act(() => new Promise((done) => setTimeout(done, 50)));
    expect(fake.previewed).toEqual([want()]);
  });
});

describe('the previews and Screen Recording', () => {
  it('with_screen_recording_not_allowed_the_step_shows_and_cards_keep_their_icons', async () => {
    const dialog = await openOn(recorderSetup({ permissions: { screen: 'notAsked' } }));
    // No screen picture is asked for; the camera and the mic still are.
    expect(fake.previewed).toEqual([want({ thumbs: false, live: null })]);
    expect(dialog).toHaveTextContent('Klipolo needs your permission to record your screen.');
    await sendFrame(fake, 'thumb', 'display:main', 'main 1');
    await sendFrame(fake, 'live', 'display:main', 'live 1');
    expect(shownIn(card('Display main'))).toBeNull();
    expect(live()).toBeNull();
  });

  it('screen_pictures_the_system_refuses_leave_the_icons_and_are_not_asked_for_again', async () => {
    await openOn(recorderSetup({ displays: [display('main'), display('studio')] }));
    await sendFrame(fake, 'thumb', 'display:main', 'main 1');
    emit('preview-thumbs', failed('capture.permission.screen'));
    await flush();
    expect(shownIn(card('Display main'))).toBeNull();
    expect(fake.previewed.at(-1)).toEqual(want({ thumbs: false, live: null }));
    // Nor for a change of choice.
    await userEvent.click(card('Display studio'));
    await flush();
    expect(calls().at(-1)).toEqual(want({ thumbs: false, live: null }));
  });

  it('the_app_refusing_the_screen_pictures_leaves_the_icons', async () => {
    fake.script.previews = (w) => (w.thumbs ? refusal('capture.permission.screen') : undefined);
    await openOn();
    await flush();
    expect(fake.previewed).toEqual([want(), want({ thumbs: false, live: null })]);
  });

  it('previews_come_back_once_a_cancelled_take_has_let_go', async () => {
    await openOn();
    await userEvent.click(recordButton());
    act(() => fake.emit('take-1', { ev: 'countdown', leftS: 3 }));
    // The app is busy until the take's worker has gone.
    fake.script.previews = () => refusal('capture.busy', { place: { code: 'capture.place.menuBar', params: {} } });
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await flush();
    const asked = calls().length;
    fake.script.previews = undefined;
    await vi.waitFor(() => expect(calls().length).toBe(asked + 1), { timeout: 3000 });
    expect(calls().at(-1)).toEqual(want());
  });

});
