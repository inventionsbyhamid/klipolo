import type { AppInfo } from '../bindings/AppInfo';
import type { ArmedTake } from '../bindings/ArmedTake';
import type { ArmOptions } from '../bindings/ArmOptions';
import type { CaptureCommand } from '../bindings/CaptureCommand';
import type { CaptureEvent } from '../bindings/CaptureEvent';
import type { CodedError } from '../bindings/CodedError';
import type { DeviceInfo } from '../bindings/DeviceInfo';
import type { DisplayInfo } from '../bindings/DisplayInfo';
import type { Permissions } from '../bindings/Permissions';
import type { PreviewKind } from '../bindings/PreviewKind';
import type { Previews } from '../bindings/Previews';
import type { RecorderEvent } from '../bindings/RecorderEvent';
import type { RecorderSetup } from '../bindings/RecorderSetup';
import type { Snapshot } from '../bindings/Snapshot';
import type { StoppedTake } from '../bindings/StoppedTake';
import { codedMessage } from '../bridge/call';
import { installRecorderBridge, type RecorderBridge, type TakeImport } from '../bridge/recorder';
import { followRecorder, forgetRecorder } from '../state/recorder';

/** A display as the setup lists it: 2560 x 1440 at 60 Hz unless `over` says otherwise. */
export function display(id: string, over: Partial<DisplayInfo> = {}): DisplayInfo {
  return { id, name: `Display ${id}`, width: 2560, height: 1440, refreshHz: 60, ...over };
}

/** A built-in camera or mic, free and awake, named after its id. */
export function device(id: string, over: Partial<DeviceInfo> = {}): DeviceInfo {
  return { id, name: id, transport: 'builtIn', isVirtual: false, inUse: false, suspended: false, ...over };
}

/** A running app with windows. */
export function app(appId: string, name = appId): AppInfo {
  return { appId, name, pid: 100, isGame: false, frontmost: false, exe: null };
}

/**
 * What a test changes in the setup: `displays`, `apps`, `mics` and `cameras` are the devices of that kind (the
 * setup's own per-display offers stay empty), `permissions` the ones named; anything else as given.
 */
export type SetupOver = Partial<Omit<RecorderSetup, 'permissions' | 'displays'>> & {
  displays?: DisplayInfo[];
  apps?: AppInfo[];
  mics?: DeviceInfo[];
  cameras?: DeviceInfo[];
  permissions?: Partial<Permissions>;
};

/**
 * The notes the app's setup adds about the default devices' permissions, worded as it words them: a refused
 * microphone while one is picked, and a refused camera while the computer has one.
 */
function permissionNotes(setup: RecorderSetup): CodedError[] {
  const notes: CodedError[] = [];
  if (setup.mic !== null && setup.permissions.mic === 'denied') notes.push({ code: 'capture.permission.mic', params: {} });
  if (setup.devices.cameras.length > 0 && setup.permissions.camera === 'denied')
    notes.push({ code: 'capture.permission.camera', params: {} });
  return notes;
}

/**
 * A Mac's setup: one display (`main`), one mic (`mic-1`, the default), no camera and no app, every permission granted,
 * nothing running. Its notes are the test's own, then those the app adds for a refused microphone or camera.
 */
export function recorderSetup(over: SetupOver = {}): RecorderSetup {
  const setup = setupOf(over);
  return { ...setup, notes: [...setup.notes, ...permissionNotes(setup)] };
}

function setupOf(over: SetupOver): RecorderSetup {
  const { displays = [display('main')], apps = [], mics = [device('mic-1')], cameras = [], permissions, ...rest } = over;
  return {
    caps: {
      screen: 'screenCaptureKit',
      appAudio: 'screenCaptureKit',
      hotkeys: 'carbon',
      focusEvents: true,
      picker: 'ourList',
      hud: true,
      osSupported: true,
      indicator: 'menuBar',
      cursor: { hide: true, positions: true, shapes: true, clicks: 'events' },
    },
    devices: { displays, apps, cameras, mics, output: null },
    permissions: { screen: 'granted', camera: 'granted', mic: 'granted', ...permissions },
    choices: { screen: null, native: null, available: [], camera: null, cameraNote: null },
    offers: [],
    displays: [],
    mic: mics[0]?.id ?? null,
    camera: cameras[0]?.id ?? null,
    mirrorCamera: true,
    freeBytes: null,
    notes: [],
    busy: false,
    ownTake: false,
    privacySettings: true,
    screenRefusedAfterReopen: false,
    suggestedGame: null,
    ...rest,
  };
}

/** The app's refusal as `call` gives it: an Error in the code's words, with the code as its cause. */
export function refusal(code: string, params: CodedError['params'] = {}): Error {
  const said: CodedError = { code, params };
  return new Error(codedMessage(said), { cause: said });
}

/** A promise a test settles itself (an answer held until the test says). */
export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

/** A take that went into the project `snapshot`, its clip `clipId`. */
export function takeImport(snapshot: Snapshot, clipId: string, over: Partial<TakeImport> = {}): TakeImport {
  return { take: 'take-1', recordingId: 'rec-1', clipId, snapshot, notes: [], zooms: 0, ...over };
}

/** An answer, or the Error it is refused with. */
type Answer<T> = T | Error | Promise<T>;

/** How the fake answers. Each can be changed while a test runs (`fake.script`). */
export interface FakeRecorderScript {
  /** The setup; `recorderSetup()` by default. */
  setup?: () => Answer<RecorderSetup>;
  /** Arming; `take-1`, `take-2`, ... with a 3 s countdown by default. */
  arm?: (options: ArmOptions) => Answer<ArmedTake>;
  /** A command to a take; taken by default. */
  send?: (take: string, command: CaptureCommand) => Answer<void> | undefined;
  stopRunning?: () => Answer<StoppedTake | null>;
  allowScreen?: () => Answer<void> | undefined;
  reopen?: () => Answer<void> | undefined;
  openPrivacy?: () => Answer<void> | undefined;
  /** The previews, as wanted: run by default. */
  previews?: (want: Previews) => Answer<void> | undefined;
  /**
   * A take that is cancelled or disarmed ends with its `state: idle` right after, as the app says once its worker has
   * gone (true by default). False: the test says it (`emit`).
   */
  idle?: boolean;
}

/** A fake recorder, what it was asked, and the events it can send. */
export type FakeRecorder = RecorderBridge & {
  script: FakeRecorderScript;
  /** Every call in order: `setup`, `arm`, `send:start`, `stopRunning`, `allowScreen`, `reopen`, `openPrivacy:mic`. */
  calls: string[];
  /** What each arming asked for. */
  armed: ArmOptions[];
  /** Every command sent, with its take. */
  sent: { take: string; command: CaptureCommand }[];
  /** Every previews call in order: what was wanted, or `off`. */
  previewed: (Previews | 'off')[];
  /** Each frame said drawn, by its kind, in order. */
  acks: PreviewKind[];
  /** Every call in order, the previews' with the rest (`previews`, `previewsOff`; acks aside). */
  log: string[];
  /** Where the app sends preview frames: the latest previews call's channel; null before the first. */
  frames: ((message: ArrayBuffer) => void) | null;
  /** Take `take`'s worker (or the app about it) says `event`. */
  emit: (take: string, event: CaptureEvent) => void;
  /** A take went into a project. */
  land: (landed: TakeImport) => void;
};

async function answer<T>(given: Answer<T> | undefined, fallback: () => T): Promise<T> {
  const value = await (given === undefined ? fallback() : given);
  if (value instanceof Error) throw value;
  return value;
}

/** A recorder that captures nothing: it answers as `script` says and keeps what it was asked. */
export function fakeRecorder(script: FakeRecorderScript = {}): FakeRecorder {
  const events = new Set<(event: RecorderEvent) => void>();
  const landings = new Set<(landed: TakeImport) => void>();
  let takes = 0;
  const listen = <T>(handlers: Set<(value: T) => void>, handler: (value: T) => void) => {
    handlers.add(handler);
    return Promise.resolve(() => void handlers.delete(handler));
  };
  /** A call, in both lists. */
  const record = (call: string) => {
    fake.calls.push(call);
    fake.log.push(call);
  };
  const fake: FakeRecorder = {
    script,
    calls: [],
    armed: [],
    sent: [],
    previewed: [],
    acks: [],
    log: [],
    frames: null,
    setup: () => {
      record('setup');
      return answer(fake.script.setup?.(), () => recorderSetup());
    },
    arm: (options) => {
      record('arm');
      fake.armed.push(options);
      takes += 1;
      const id = `take-${takes}`;
      return answer(fake.script.arm?.(options), () => ({ id, countdownS: 3 }));
    },
    send: async (take, command) => {
      record(`send:${command.cmd}`);
      fake.sent.push({ take, command });
      await answer(fake.script.send?.(take, command), () => undefined);
      if ((command.cmd === 'cancel' || command.cmd === 'disarm') && fake.script.idle !== false)
        queueMicrotask(() => fake.emit(take, { ev: 'state', state: 'idle', elapsedUs: 0 }));
    },
    stopRunning: () => {
      record('stopRunning');
      return answer(fake.script.stopRunning?.(), () => null);
    },
    allowScreen: () => {
      record('allowScreen');
      return answer(fake.script.allowScreen?.(), () => undefined);
    },
    reopen: () => {
      record('reopen');
      return answer(fake.script.reopen?.(), () => undefined);
    },
    openPrivacy: (page) => {
      record(`openPrivacy:${page}`);
      return answer(fake.script.openPrivacy?.(), () => undefined);
    },
    previews: (want, onFrame) => {
      fake.log.push('previews');
      fake.previewed.push(want);
      fake.frames = onFrame;
      return answer(fake.script.previews?.(want), () => undefined);
    },
    previewsOff: () => {
      fake.log.push('previewsOff');
      fake.previewed.push('off');
      return Promise.resolve();
    },
    previewAck: (kind) => {
      fake.acks.push(kind);
      return Promise.resolve();
    },
    onEvent: (handler) => listen(events, handler),
    onImported: (handler) => listen(landings, handler),
    emit: (take, event) => {
      for (const handler of [...events]) handler({ take, event });
    },
    land: (landed) => {
      for (const handler of [...landings]) handler(landed);
    },
  };
  return fake;
}

/**
 * A fresh recorder store on a new fake: the store forgets the last test's dialog and take, the fake is installed, and
 * the store follows its events, as the app does at launch.
 */
export async function installFakeRecorder(script: FakeRecorderScript = {}): Promise<FakeRecorder> {
  forgetRecorder();
  const fake = fakeRecorder(script);
  installRecorderBridge(fake);
  await followRecorder();
  return fake;
}
