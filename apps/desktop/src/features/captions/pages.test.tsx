/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, renderHook, waitFor } from '@testing-library/react';
import { emit } from '@tauri-apps/api/event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it } from 'vitest';
import { useProject } from '../../state/project';
import { page, resetCaptions, word } from '../../test/captionsBackend';
import { useAllCaptionPages } from './useCaptionPages';

const S = 1_000_000;
let asked: any[] = [];

/** `captions_pages` over `total` one-word pages at revision `revision()`, page `i` saying `say(i)`. */
function pages(total: number, revision: (query: any) => number = () => 3, say = (i: number) => `cue ${i}`) {
  return (args: any) => {
    asked.push(args);
    const { from, count } = args.query;
    const list = Array.from({ length: Math.max(0, Math.min(count, total - from)) }, (_, k) =>
      page(from + k, [word(say(from + k), (from + k) * 2.4)]),
    );
    return { revision: revision(args), format: 'portrait', total, first: from, capped: false, pages: list };
  };
}

function backend(answer: (args: any) => unknown, events = false) {
  asked = [];
  mockIPC((cmd, args) => (cmd === 'captions_pages' ? answer(args) : null), { shouldMockEvents: events });
}

const enable = () => {
  const snap = useProject.getState().snapshot!;
  useProject.setState({ snapshot: { ...snap, project: { ...snap.project, captions: { ...snap.project.captions, enabled: true } } } });
};

const toRevision = (revision: number) =>
  act(() => {
    const snap = useProject.getState().snapshot!;
    useProject.setState({ snapshot: { ...snap, history: { ...snap.history, revision } } });
  });

beforeEach(async () => {
  await resetCaptions();
  enable();
});

describe('the Captions view’s pages', () => {
  it('asks for them again when what was said changes (speech-changed)', async () => {
    backend(pages(3), true);
    const { result } = renderHook(() => useAllCaptionPages('portrait'));
    await waitFor(() => expect(result.current.view?.pages).toHaveLength(3));
    expect(asked).toHaveLength(1);
    await act(() => emit('speech-changed'));
    await waitFor(() => expect(asked).toHaveLength(2));
  });

  it('keeps a page that reads the same as the one before, so its line isn’t drawn again', async () => {
    let fixed = false;
    backend(pages(3, () => 3, (i) => (i === 1 && fixed ? 'fixed' : `cue ${i}`)));
    const { result } = renderHook(() => useAllCaptionPages('portrait'));
    await waitFor(() => expect(result.current.view?.pages).toHaveLength(3));
    const before = result.current.view!.pages;
    fixed = true;
    toRevision(4);
    await waitFor(() => expect(result.current.view!.pages[1].words[0].text).toBe('fixed'));
    const after = result.current.view!.pages;
    expect(after[0]).toBe(before[0]);
    expect(after[2]).toBe(before[2]);
    expect(after[1]).not.toBe(before[1]);
  });

  it('asks again when the chunks of one answer come from different revisions', async () => {
    let raced = true;
    // The second chunk is answered after an edit landed in Rust, once.
    backend(
      pages(450, (args) => {
        if (args.query.from === 200 && raced) {
          raced = false;
          return 4;
        }
        return 3;
      }),
    );
    const { result } = renderHook(() => useAllCaptionPages('portrait'));
    await waitFor(() => expect(result.current.view?.pages).toHaveLength(450));
    // Three chunks, then all three again.
    expect(asked).toHaveLength(6);
    expect(result.current.view!.revision).toBe(3);
  });

  it('keeps every chunk of a long list: a view shown again asks for nothing (past 800 pages)', async () => {
    backend(pages(1000));
    const first = renderHook(() => useAllCaptionPages('portrait'));
    await waitFor(() => expect(first.result.current.view?.pages).toHaveLength(1000));
    expect(asked).toHaveLength(5);
    first.unmount();
    const again = renderHook(() => useAllCaptionPages('portrait'));
    await waitFor(() => expect(again.result.current.view?.pages).toHaveLength(1000));
    expect(asked).toHaveLength(5);
    expect(again.result.current.view!.pages[999].startUs).toBe(999 * 2.4 * S);
  });
});
