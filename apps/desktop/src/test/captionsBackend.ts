/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments, as the captions tests read them */
import { waitFor } from '@testing-library/react';
import { mockIPC } from '@tauri-apps/api/mocks';
import { expect } from 'vitest';
import type { Snapshot } from '../bindings/Snapshot';
import { forgetCaptionPages } from '../features/captions/useCaptionPages';
import { activateLocale } from '../i18n';
import { useCaptionsView } from '../state/captionsView';
import { clearPreview } from '../state/livePreview';
import { usePlayback } from '../state/playback';
import { useProject } from '../state/project';
import { useSpeech } from '../state/speech';
import { useUi } from '../state/ui';
import { fixtureSnapshot, revisionPatch } from './fixtures';

/**
 * The captions tests' Rust side: every IPC call is kept in `calls`; `project_apply` answers the next revision and
 * `captions_pages` 40 pages of one word each, unless `extra` answers a command otherwise.
 */
export type Call = { cmd: string; args: any };
export let calls: Call[] = [];
export const S = 1_000_000;

/** A caption word for the mocked pages. */
export function word(text: string, at: number, extra: Record<string, unknown> = {}) {
  return {
    text,
    joined: false,
    startUs: at * S,
    endUs: (at + 0.3) * S,
    assetId: 'mic',
    sourceStartUs: at * S,
    sourceEndUs: (at + 0.3) * S,
    look: {},
    keyword: null,
    ...extra,
  };
}

/** Page `i`: "cue i", one word, 2 s long, 2.4 s apart. */
export const page = (i: number, words?: ReturnType<typeof word>[]) => ({
  index: i,
  startUs: i * 2.4 * S,
  endUs: (i * 2.4 + 2) * S,
  words: words ?? [
    {
      ...word(`cue ${i}`, i * 2.4),
      endUs: (i * 2.4 + 1) * S,
      sourceStartUs: i * S,
      sourceEndUs: (i + 1) * S,
    },
  ],
});

/** The answer `captions_pages` gives by default. */
export const pagesAnswer = (pages = Array.from({ length: 40 }, (_, i) => page(i)), capped = false) => ({
  revision: 0,
  format: 'portrait',
  total: pages.length,
  first: 0,
  capped,
  pages,
});

/** The fixture with a 9:16 version too (the preview shows 9:16 in these tests). */
export function captionsSnapshot(enabled = false): Snapshot {
  const snap = fixtureSnapshot();
  snap.project.settings = { ...snap.project.settings, formats: ['landscape', 'portrait'] };
  if (enabled) snap.project.captions = { ...snap.project.captions, enabled: true };
  return snap;
}

export function captionsBackend(extra: Record<string, (args: any) => unknown> = {}) {
  calls = [];
  const snap = fixtureSnapshot();
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    if (extra[cmd]) return extra[cmd](args);
    if (cmd === 'project_apply') return revisionPatch(snap.history.revision + 1);
    if (cmd === 'captions_pages') return pagesAnswer();
    return null;
  });
}

/** Every test starts in English with the 9:16 preview, nothing kept and the playhead at 0. */
export async function resetCaptions() {
  await activateLocale('en');
  useProject.setState({ snapshot: captionsSnapshot() });
  useSpeech.setState({ pagesEpoch: 0 });
  forgetCaptionPages();
  // A dry run another test left sent: the same ops on the same revision would be skipped.
  clearPreview();
  useUi.setState({ previewFormat: 'portrait', pxPerSecond: 40 });
  useCaptionsView.setState({ open: false });
  usePlayback.setState({ playheadUs: 0, playing: false });
}

/** The transactions sent. */
export const applied = () => calls.filter((c) => c.cmd === 'project_apply').map((c) => c.args.tx);
/** The calls of `cmd` made. */
export const callsOf = (cmd: string) => calls.filter((c) => c.cmd === cmd);

/** The transactions sent, once the commit queue has sent `n`. */
export async function landed(n = 1) {
  await waitFor(() => expect(applied().length).toBeGreaterThanOrEqual(n));
  return applied();
}
