import { Channel } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { ArmedTake } from '../bindings/ArmedTake';
import type { BackFromReopen } from '../bindings/BackFromReopen';
import type { ArmOptions } from '../bindings/ArmOptions';
import type { CaptureCommand } from '../bindings/CaptureCommand';
import type { CodedError } from '../bindings/CodedError';
import type { PreviewKind } from '../bindings/PreviewKind';
import type { Previews } from '../bindings/Previews';
import type { PrivacyPage } from '../bindings/PrivacyPage';
import type { RecorderEvent } from '../bindings/RecorderEvent';
import type { RecorderSetup } from '../bindings/RecorderSetup';
import type { Snapshot } from '../bindings/Snapshot';
import type { StoppedTake } from '../bindings/StoppedTake';
import { call } from './call';

export type { UnlistenFn };

/** A finished take that went into a project (`recorder-imported`): the project as it is now, with the take's clip. */
export interface TakeImport {
  /** The take's id (`ArmedTake.id`). */
  take: string;
  recordingId: string;
  /** The take's clip, last on the main track. */
  clipId: string;
  /** The project it went into: the open one, or the one made for it when none was open. */
  snapshot: Snapshot;
  /** What the person should know about the take (a source that didn't start, a gap). */
  notes: CodedError[];
  /** How many auto zooms were added after it, as their own undo step. */
  zooms: number;
  /**
   * An earlier run of Klipolo left the take behind (it quit or crashed during it), and it landed in a new project as
   * Klipolo opened again.
   */
  earlierRun: boolean;
}

/**
 * The recorder's commands and events (`crates/klipolo-app/src/recorder/commands.rs`). Opening the Record dialog and
 * reading its setup capture nothing and ask nothing: capture starts only with `send(take, { cmd: 'start' })` on the
 * person's own Record press, and Screen Recording is asked for only on their "Allow screen recording". Chat and MCP
 * never reach these.
 *
 * Every command names its take (`ArmedTake.id`) and every event carries one (`RecorderEvent.take`), so nothing meant
 * for an earlier take reaches a newer one: the app refuses a command for a take that has ended with
 * `capture.error.staleTake`. A failure is an Error with one plain sentence, keeping the code as its cause (`call`).
 */
export interface RecorderBridge {
  /** Devices, permissions, each display's qualities, free space and notes. No capture, no prompt. */
  setup(): Promise<RecorderSetup>;
  /** Launches the take's worker, armed: nothing is captured until Start. `capture.busy` while a take runs. */
  arm(options: ArmOptions): Promise<ArmedTake>;
  /** One command to take `take`: Start, Pause, Resume, Stop, Cancel, Disarm or WithoutSource. */
  send(take: string, command: CaptureCommand): Promise<void>;
  /**
   * "Stop the running recording": Stop for whichever take the app runs, whatever id the page has for it. Its id and
   * whether the app saw it start, or null when none runs. The take's own events say how it ends.
   */
  stopRunning(): Promise<StoppedTake | null>;
  /** "Allow screen recording", on the person's press alone: macOS asks, then its settings page opens. */
  allowScreen(): Promise<void>;
  /**
   * "Reopen Klipolo": the project is saved, then Klipolo starts again, which applies the grant. It resolves before
   * Klipolo has gone. Refused with `capture.busy` during a take.
   */
  reopen(): Promise<void>;
  /**
   * Klipolo opened just after Reopen Klipolo: the project open then (null on the start screen), to open again with the
   * Record dialog. Once per run; null otherwise.
   */
  backFromReopen(): Promise<BackFromReopen | null>;
  /**
   * At launch, once the page follows the recorder: takes an earlier run left behind (Klipolo quit or crashed during
   * them) land, each in a new project, and come as `onImported` with `earlierRun`. It resolves at once; the app looks
   * once per run, and not at all in a test run.
   */
  recoverLeft(): Promise<void>;
  /** "Open privacy settings": the Screen Recording, camera or microphone page. It only opens the page. */
  openPrivacy(page: PrivacyPage): Promise<void>;
  /**
   * The Record dialog's previews, as `want` says (`recorder_previews`): the app starts and stops them to match, within
   * the consent rules (the screen's pictures only with Screen Recording allowed; the camera and the mic only when
   * allowed, or on the person's own Allow press, `want.ask`). Each picture comes to `onFrame` as the channel's raw
   * message (`previewFrame`); the mic's level and why a preview stopped come as `recorder-event`s of a `PreviewId`.
   * Refused with `capture.permission.screen` (the camera and the mic were still set), `capture.busy` while a take is
   * the app's, and `capture.error.testRun` in a test run. While Klipolo is behind it starts nothing and resolves.
   */
  previews(want: Previews, onFrame: (message: ArrayBuffer) => void): Promise<void>;
  /** Every preview stops (`recorder_previews_off`). */
  previewsOff(): Promise<void>;
  /**
   * A frame of `kind` reached the page (`recorder_preview_ack`): the app sends a kind's next frames only while at
   * most two are unanswered, so a slow page never piles them up.
   */
  previewAck(kind: PreviewKind): Promise<void>;
  /** Every take's events (`recorder-event`), each with its take's id, until the returned function is called. */
  onEvent(handler: (event: RecorderEvent) => void): Promise<UnlistenFn>;
  /** Every take that went into a project (`recorder-imported`), until the returned function is called. */
  onImported(handler: (landed: TakeImport) => void): Promise<UnlistenFn>;
}

/** The app's recorder. */
export const tauriRecorder: RecorderBridge = {
  setup: () => call<RecorderSetup>('recorder_setup'),
  arm: (options) => call<ArmedTake>('recorder_arm', { options }),
  send: (take, command) => call<void>('recorder_send', { take, command }),
  stopRunning: () => call<StoppedTake | null>('recorder_stop_running'),
  allowScreen: () => call<void>('recorder_allow_screen'),
  reopen: () => call<void>('recorder_reopen'),
  backFromReopen: () => call<BackFromReopen | null>('recorder_back_from_reopen'),
  recoverLeft: () => call<void>('recorder_recover_left'),
  openPrivacy: (page) => call<void>('recorder_open_privacy', { page }),
  previews: (want, onFrame) => call<void>('recorder_previews', { want, frames: new Channel<ArrayBuffer>(onFrame) }),
  previewsOff: () => call<void>('recorder_previews_off'),
  previewAck: (kind) => call<void>('recorder_preview_ack', { kind }),
  onEvent: (handler) => listen<RecorderEvent>('recorder-event', (event) => handler(event.payload)),
  onImported: (handler) => listen<TakeImport>('recorder-imported', (event) => handler(event.payload)),
};

let current: RecorderBridge = tauriRecorder;

/** The recorder in use: the app's, unless a fake was installed (`installRecorderBridge`). */
export const recorder: RecorderBridge = {
  setup: () => current.setup(),
  arm: (options) => current.arm(options),
  send: (take, command) => current.send(take, command),
  stopRunning: () => current.stopRunning(),
  allowScreen: () => current.allowScreen(),
  reopen: () => current.reopen(),
  backFromReopen: () => current.backFromReopen(),
  recoverLeft: () => current.recoverLeft(),
  openPrivacy: (page) => current.openPrivacy(page),
  previews: (want, onFrame) => current.previews(want, onFrame),
  previewsOff: () => current.previewsOff(),
  previewAck: (kind) => current.previewAck(kind),
  onEvent: (handler) => current.onEvent(handler),
  onImported: (handler) => current.onImported(handler),
};

/** Uses `bridge` for the recorder from now on (tests); `tauriRecorder` puts the app's back. */
export function installRecorderBridge(bridge: RecorderBridge) {
  current = bridge;
}
