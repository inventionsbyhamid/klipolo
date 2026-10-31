/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { followEditorView } from './editorView';
import { usePlayback } from './playback';
import { useProject } from './project';
import { useRange } from './range';
import { useSelection } from './selection';
import { useUi } from './ui';

const S = 1_000_000;
let views: any[] = [];
let stop: () => void = () => undefined;

/** Lets the debounce run out and the command go. */
async function wait(ms: number) {
  await vi.advanceTimersByTimeAsync(ms);
}

beforeEach(() => {
  vi.useFakeTimers();
  views = [];
  mockIPC((cmd, args) => {
    if (cmd === 'editor_view') views.push((args as any).view);
    return null;
  });
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useRange.setState({ range: null });
  useSelection.setState({ selectedId: null });
  useUi.setState({ previewFormat: null });
  usePlayback.setState({ playheadUs: 0, playing: false });
  stop = followEditorView();
});

afterEach(async () => {
  stop();
  await vi.runOnlyPendingTimersAsync();
  vi.useRealTimers();
});

describe('the view other apps read', () => {
  it('view_is_sent_once_after_the_selection_settles', async () => {
    // A drag: the range moves every few milliseconds.
    for (let end = 2; end <= 6; end++) {
      useRange.getState().select(1 * S, end * S);
      await wait(40);
    }
    expect(views).toEqual([]);
    await wait(250);
    expect(views).toEqual([
      {
        selection: { start: 1 * S, end: 6 * S },
        playheadUs: 0,
        item: null,
        format: fixtureSnapshot().project.settings.primaryFormat,
      },
    ]);
    // Nothing changed since: nothing more goes.
    await wait(1000);
    expect(views).toHaveLength(1);
  });

  it('a_seek_or_a_pause_sends_the_playhead_but_playing_does_not', async () => {
    usePlayback.setState({ playheadUs: 4 * S, seq: usePlayback.getState().seq + 1 });
    await wait(250);
    expect(views.map((v) => v.playheadUs)).toEqual([4 * S]);
    // Playing: the playhead runs on its own, nothing goes.
    usePlayback.setState({ playing: true });
    for (let i = 1; i <= 20; i++) {
      usePlayback.setState({ playheadUs: 4 * S + i * 40_000 });
      await wait(40);
    }
    await wait(250);
    expect(views).toHaveLength(1);
    // Paused where the engine stopped.
    usePlayback.setState({ playing: false, playheadUs: 5 * S });
    await wait(250);
    expect(views.map((v) => v.playheadUs)).toEqual([4 * S, 5 * S]);
  });

  it('another_project_is_told_its_view_again', async () => {
    useUi.setState({ previewFormat: 'portrait' });
    await wait(250);
    expect(views.at(-1)).toMatchObject({ format: 'portrait' });
    // Rust forgets the view on every opening: the page says it again once the project is open.
    useProject.setState({ snapshot: { ...fixtureSnapshot(), opening: 2 } });
    await wait(250);
    expect(views).toHaveLength(2);
    // No project: nothing to say.
    useProject.setState({ snapshot: null });
    useRange.getState().select(0, 2 * S);
    await wait(250);
    expect(views).toHaveLength(2);
  });
});
