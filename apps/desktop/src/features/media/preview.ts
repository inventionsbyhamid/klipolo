import { t } from '@lingui/core/macro';
import { create } from 'zustand';
import { library } from '../../bridge/library';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { toast } from '../../state/toasts';
import { opening } from './shelves';

/*
 * Hearing a sound before adding it: a Sounds or Music row's play button plays the pack's sound or music bed, or one of
 * the person's own sounds, through an `Audio` element on a blob URL of the WAV Rust decodes (`media_sound`: the first
 * 30 s at most). One plays at a time. Starting one pauses the timeline; the timeline starting, the item going onto the
 * timeline, the tab changing, the panel folding and another project opening each stop it.
 */

/** What a preview plays: a pack sound or music bed by its id, or a sound the open project uses by its path. */
export type SoundFrom = { packId: string } | { path: string };

/** The longest a preview plays: Rust decodes no more than a song's first 30 s. */
export const PREVIEW_MAX_US = 30_000_000;

interface PreviewStore {
  /** The item asked to play, by its mark key: none while nothing does. */
  key: string | null;
  /** Its sound has started: the row's progress runs from then. */
  sounding: boolean;
}

export const usePreview = create<PreviewStore>(() => ({ key: null, sounding: false }));

/** The element playing now. */
let audio: HTMLAudioElement | null = null;
/** Bumped by every start and stop: a sound that arrives after a later press is dropped. */
let turn = 0;

/** How many decoded sounds stay as blob URLs: a second press plays at once, and a long song's WAV doesn't pile up. */
const KEPT = 8;
/** The sounds asked for, by key, the most recent last: a blob URL once it is here. One that couldn't be had isn't kept. */
const sounds = new Map<string, Promise<string>>();

function soundUrl(key: string, from: SoundFrom): Promise<string> {
  const kept = sounds.get(key);
  if (kept) {
    sounds.delete(key);
    sounds.set(key, kept);
    return kept;
  }
  const asked = library.sound(from).then((bytes) => URL.createObjectURL(new Blob([bytes], { type: 'audio/wav' })));
  // One that couldn't be had goes, unless a newer ask took its place.
  asked.catch(() => {
    if (sounds.get(key) === asked) sounds.delete(key);
  });
  sounds.set(key, asked);
  for (const [old, url] of sounds) {
    if (sounds.size <= KEPT) break;
    sounds.delete(old);
    void url.then(URL.revokeObjectURL, () => undefined);
  }
  return asked;
}

/** Lets every kept sound go (their blob URLs revoked): another project opened. */
function forgetSounds() {
  for (const url of sounds.values()) void url.then(URL.revokeObjectURL, () => undefined);
  sounds.clear();
}

/** Stops the preview playing, if one is. */
export function stopPreview() {
  turn += 1;
  if (audio) {
    audio.pause();
    audio = null;
  }
  if (usePreview.getState().key !== null) usePreview.setState({ key: null, sounding: false });
}

const couldntPlay = () => toast.error(t({ id: 'library.preview.failed', message: 'That sound couldn’t be played.' }));

/**
 * Plays item `key` (`from`) from its start, or stops it when it is the one playing. Another preview stops first, and
 * the timeline pauses.
 */
export async function togglePreview(key: string, from: SoundFrom): Promise<void> {
  if (usePreview.getState().key === key) {
    stopPreview();
    return;
  }
  stopPreview();
  const mine = turn;
  usePlayback.getState().pause();
  usePreview.setState({ key, sounding: false });
  let url: string;
  try {
    url = await soundUrl(key, from);
  } catch {
    if (mine !== turn) return;
    stopPreview();
    couldntPlay();
    return;
  }
  if (mine !== turn) return;
  const element = new Audio(url);
  audio = element;
  const ours = () => audio === element;
  element.addEventListener('ended', () => {
    if (ours()) stopPreview();
  });
  element.addEventListener('error', () => {
    if (!ours()) return;
    stopPreview();
    couldntPlay();
  });
  try {
    await element.play();
  } catch {
    if (!ours()) return;
    stopPreview();
    couldntPlay();
    return;
  }
  if (ours()) usePreview.setState({ sounding: true });
}

// The timeline starting stops a preview, and another project's opening lets its sounds go.
usePlayback.subscribe((s, prev) => {
  if (s.playing && !prev.playing) stopPreview();
});
useProject.subscribe((s, prev) => {
  if (opening(s) === opening(prev)) return;
  stopPreview();
  forgetSounds();
});
