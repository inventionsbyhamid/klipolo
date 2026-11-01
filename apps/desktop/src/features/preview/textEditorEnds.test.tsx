import { act, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { activateLocale } from '../../i18n';
import { flushBursts } from '../../state/bursts';
import { beginCommit } from '../../state/commitQueue';
import { framesShown, noteFrameShown } from '../../state/framesShown';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { useToasts } from '../../state/toasts';
import { useUi } from '../../state/ui';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { editor, heard, rustForText, titleBox } from '../../test/textEdit';
import { Stage } from './Stage';
import { resendEditing, useTextEdit } from './textEdit';

const MOVED = 'The video changed while you typed. Your words are on the clipboard.';

beforeAll(() => {
  // jsdom has no layout: give the stage an 800×450 box.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 450 });
});

beforeEach(async () => {
  await activateLocale('en');
  heard.reset();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false, seq: 0 });
  useUi.setState({ previewFormat: null });
  useToasts.setState({ toasts: [] });
  useTextEdit.setState({ session: null, closing: null });
  rustForText();
});

afterEach(() => vi.useRealTimers());

/** Begins typing the fixture title on the canvas and adds `typed` at its end. */
async function typeOnTitle(typed: string) {
  renderWithProviders(<Stage />);
  await userEvent.dblClick(titleBox());
  await userEvent.type(editor(), typed);
}

describe('how a canvas edit ends', () => {
  it('the look stays until the preview has shown two frames since the title came back', async () => {
    await typeOnTitle(' now');
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(heard.calls).toEqual(['hide title', 'apply', 'show']));
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(useTextEdit.getState().closing, 'no frame yet: the look stays').not.toBeNull();
    act(() => noteFrameShown());
    expect(useTextEdit.getState().closing, 'one frame may still be the hidden still').not.toBeNull();
    act(() => noteFrameShown());
    await waitFor(() => expect(useTextEdit.getState().closing).toBeNull());
  });

  it('waiting for frames ends at its cap when the preview draws none', async () => {
    vi.useFakeTimers();
    let done = false;
    void framesShown(2, 200).then(() => (done = true));
    noteFrameShown();
    await vi.advanceTimersByTimeAsync(199);
    expect(done).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(done).toBe(true);
  });

  it('a read-only project begins no edit', async () => {
    useProject.setState({ snapshot: { ...fixtureSnapshot(), readOnly: 'model.readOnly' } });
    renderWithProviders(<Stage />);
    await userEvent.dblClick(titleBox());
    expect(useTextEdit.getState().session).toBeNull();
    expect(heard.editing).toEqual([]);
  });

  it('a commit that fails for another reason keeps the words without saying the video changed', async () => {
    const write = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: write }, configurable: true });
    rustForText(() => Promise.reject({ code: 'project.damaged', params: {} }));
    await typeOnTitle(' now');
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(write).toHaveBeenCalledWith('Set up billing now'));
    const said = useToasts.getState().toasts.map((t) => t.text);
    expect(said).not.toContain(MOVED);
    expect(said, 'the failure is said once, by the edit itself').toHaveLength(1);
  });

  it('closing with the edit’s commit still waiting its turn keeps it, and the next project shows nothing of it', async () => {
    await typeOnTitle(' now');
    // A chat change being merged holds the commit queue as the edit finishes.
    const end = beginCommit();
    act(() => void useTextEdit.getState().finish());
    expect(heard.applied).toHaveLength(0);
    // The project closes: the edit keeps its turn (the close waits for it), built once the merge has landed (sent at
    // once, it would put back what the merge changed).
    act(() => void flushBursts({ closing: true }));
    expect(heard.applied).toHaveLength(0);
    end();
    await waitFor(() => expect(heard.applied).toHaveLength(1));
    expect(heard.applied[0].ops[0]).toMatchObject({ patch: { content: { text: 'Set up billing now' } } });
    act(() => {
      const next = fixtureSnapshot();
      useProject.setState({ snapshot: { ...next, project: { ...next.project, id: 'another' } } });
    });
    expect(useTextEdit.getState().closing).toBeNull();
  });

  it('an engine that starts while a title is typed leaves it out too', async () => {
    await typeOnTitle('!');
    expect(heard.editing).toEqual(['title']);
    act(() => resendEditing());
    expect(heard.editing).toEqual(['title', 'title']);
  });
});
