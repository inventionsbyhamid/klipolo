import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import type { ErrorParam } from '../bindings/ErrorParam';
import type { Recording } from '../bindings/Recording';
import type { RecordingSource } from '../bindings/RecordingSource';
import type { SourceRole } from '../bindings/SourceRole';
import { addLabelSource, labelDescriptor, wordLabel } from './labels';
import { isVideoRole, nthOfRole } from './recording';

/**
 * Klipolo's names for a recording's sources, by role, for a source whose label is still Klipolo's own
 * `capture.source.*` code: the same words the recorder's codes have.
 */
const ROLE_NAMES: Record<SourceRole, MessageDescriptor> = {
  screen: msg({ id: 'source.role.screen', message: 'Screen', comment: "A recording's screen picture" }),
  game: msg({ id: 'source.role.game', message: 'Game', comment: "A recording's game picture" }),
  camera: msg({ id: 'source.role.camera', message: 'Camera', comment: "A recording's camera picture" }),
  mic: msg({ id: 'source.role.mic', message: 'Mic', comment: 'The microphone recorded with a take' }),
  gameAudio: msg({ id: 'source.role.gameAudio', message: 'Game sound', comment: "The game's own sound" }),
  systemAudio: msg({
    id: 'source.role.systemAudio',
    message: 'Computer sound',
    comment: 'Everything the computer played while recording',
  }),
  voiceChat: msg({ id: 'source.role.voiceChat', message: 'Voice chat', comment: "A voice-chat app's sound" }),
  otherApp: msg({ id: 'source.role.otherApp', message: 'App sound', comment: "Another app's sound" }),
};

/** The sound a camera recorded with its picture: its lane and its row in Sounds. */
const CAMERA_SOUND = msg({
  id: 'source.cameraSound.name',
  message: 'Camera sound',
  comment: 'A lane and a row in Sounds: the sound a camera recorded with its picture',
});
/** The same for a screen or game file that carries sound. */
const VIDEO_SOUND = msg({
  id: 'source.cameraSound.ofVideo',
  message: '{video}’s sound',
  comment: 'A lane and a row in Sounds: the sound a video file recorded; {video} is its name, e.g. Screen',
});
/** A recording's second (third...) source of a kind, when Klipolo names it. */
const NTH = msg({
  id: 'source.lane.nth',
  message: '{name} {n}',
  comment: "A lane for a recording's second (third...) sound of a kind; {name} is e.g. Mic, {n} its number, e.g. 2",
});

const byId = new Map([...Object.values(ROLE_NAMES), CAMERA_SOUND, VIDEO_SOUND, NTH].map((d) => [d.id, d]));
// Undo labels name a source by these codes too (`sourceParam`): the history words them in the language shown.
addLabelSource((id) => byId.get(id));

const OWN_NAME = 'capture.source.';

/** Whether `s` still has the name Klipolo gave it (a `capture.source.*` code), not the person's own. */
export const klipoloNamed = (s: Pick<RecordingSource, 'label'>) => s.label.startsWith(OWN_NAME);

/** A role's name as a nested code: "Mic", "Computer sound". */
export const roleParam = (role: SourceRole): ErrorParam => ({ code: ROLE_NAMES[role].id, params: {} });

/**
 * A source's name as an undo label's value: the person's own name for it, or Klipolo's as a nested code, worded in the
 * language shown wherever the label is read (the Edit menu, chat's English history).
 */
export function sourceParam(s: Pick<RecordingSource, 'label' | 'role'>): ErrorParam {
  if (!klipoloNamed(s)) return s.label;
  return labelDescriptor(s.label) ? { code: s.label, params: {} } : roleParam(s.role);
}

/** A name as an undo label's value, in the language shown. */
export const paramText = (p: ErrorParam): string =>
  typeof p === 'object' ? wordLabel(p.code, p.params) : String(p);

/** A source's name in the language shown: "Mic", "Computer sound", or the person's own name for it. */
export const sourceName = (s: Pick<RecordingSource, 'label' | 'role'>): string => paramText(sourceParam(s));

/** A kind of recorded sound's name in the language shown ("Mic", "Computer sound"): Klipolo's word for its role. */
export const roleName = (role: SourceRole): string => wordLabel(ROLE_NAMES[role].id, {});

/** "Mic 2": `name` numbered as a recording's `nth` (0-based) source of its kind. */
export const nthParam = (name: ErrorParam, nth: number): ErrorParam => ({
  code: NTH.id,
  params: { name, n: nth + 1 },
});

/** "{video}’s sound": the sound a video file carries. */
export const videoSoundParam = (video: ErrorParam): ErrorParam => ({
  code: VIDEO_SOUND.id,
  params: { video },
});

/** The sound of a picture source's files: "Camera sound", or "{video}'s sound" for a screen or game file. */
export function pictureSoundParam(s: Pick<RecordingSource, 'label' | 'role'>): ErrorParam {
  return s.role === 'camera' ? { code: CAMERA_SOUND.id, params: {} } : videoSoundParam(sourceParam(s));
}

/**
 * A sound source's name as a value: a picture source's own sound as `pictureSoundParam`, any other as `sourceParam`.
 * Within recording `r`, a second (third...) source of a kind Klipolo named reads "Mic 2", as its lane does.
 */
export function soundParam(r: Recording, s: RecordingSource): ErrorParam {
  const base = isVideoRole(s.role) ? pictureSoundParam(s) : sourceParam(s);
  const nth = nthOfRole(r, s);
  return nth > 0 && klipoloNamed(s) ? nthParam(base, nth) : base;
}

/** `soundParam` in the language shown. */
export const soundName = (r: Recording, s: RecordingSource): string => paramText(soundParam(r, s));
