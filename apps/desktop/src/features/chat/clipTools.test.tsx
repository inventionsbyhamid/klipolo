/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it } from 'vitest';
import type { CodedError } from '../../bindings/CodedError';
import { useAi } from '../../ai/aiStatus';
import { installChatBridge } from '../../bridge/chat';
import { fakeChat, proposal, reply, type FakeChatScript } from '../../bridge/chatFake';
import { useAnalysis } from '../../state/analysis';
import { useChat, useChatStandIn } from '../../state/chat';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useProposal } from '../../state/proposal';
import { useRange } from '../../state/range';
import { useSelection } from '../../state/selection';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { ChatPanel } from './ChatPanel';

/*
 * Chat's clip changes in the panel: a speed change says what it did to the transitions in its reply and in its Undo,
 * as the clip card's does ("Change speed. A transition is removed."), applied at once or after Apply.
 */

let calls: Record<string, any[]> = {};

function backend(extra: Record<string, (args: any) => unknown> = {}) {
  calls = {};
  mockIPC((cmd, args) => {
    (calls[cmd] ??= []).push(args);
    if (extra[cmd]) return extra[cmd](args);
    return null;
  });
}

/** The project after a change: the next revision with `undo` as its history's top. */
function changed(revision: number, undo: CodedError | null, extra: Record<string, unknown> = {}) {
  const snapshot = fixtureSnapshot();
  return {
    ...fixturePatch(snapshot),
    baseRevision: revision - 1,
    history: {
      ...snapshot.history,
      revision,
      canUndo: undo !== null,
      undoLabel: undo?.code ?? null,
      undoParams: undo?.params,
      ...extra,
    },
  };
}

/** The undo label Rust gives a speed change of c2 that refitted transitions (`chat.setClip.note`). */
const speedNote = (removed: number, shortened: number): CodedError => ({
  code: 'chat.setClip.note',
  params: { clip: 'c2', change: 'speed', note: { code: 'timeline.refit', params: { removed, shortened } } },
});

function chat(script: Partial<FakeChatScript>) {
  const fake = fakeChat({ reply: (r) => reply(r.id, 'Done.'), ...script });
  installChatBridge(fake);
  return fake;
}

const message = () => screen.getByRole('textbox', { name: 'Message' });

beforeEach(() => {
  backend();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  useRange.setState({ range: null });
  useProposal.getState().show(null);
  usePlayback.setState({ playheadUs: 0, playing: false });
  useAi.setState({ status: { mode: 'key', keyHint: '6789' } });
  useChat.getState().reset(fixtureSnapshot().project.id);
  useChatStandIn.setState({ on: false });
  const done = { state: 'done', approximate: false } as const;
  useAnalysis.setState({ states: { screen: done, camera: done, mic: done, music: done, broll: done } });
});

describe("chat's clip changes", () => {
  it('a speed that removes a transition waits for Apply, says why, and its Undo names what it removed', async () => {
    const undo = speedNote(1, 0);
    // Rust's proposal: no footage goes, one transition does.
    const p = proposal('p1', 3, {
      title: 'Remove 1 transition',
      removals: { ...proposal('x', 0).removals, transitions: ['b'] },
    });
    const fake = chat({
      reply: (r) =>
        reply(
          r.id,
          'Here is what I would remove. Apply it or discard it. ' +
            'At that speed a transition no longer fits, so the change waits for Apply.',
          { proposal: p },
        ),
      apply: () => ({ change: changed(4, undo), undo }),
      undoLast: (revision) => changed(revision + 1, null, { canRedo: true, redoLabel: 'chat.setClip.note' }),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'speed up the middle part 2x{Enter}');
    expect(
      await screen.findByText(/At that speed a transition no longer fits, so the change waits for Apply\.$/),
    ).toBeInTheDocument();
    const card = screen.getByRole('group', { name: 'Remove 1 transition' });
    // Nothing is cut: the card says what goes, never a length.
    expect(card).toHaveTextContent('Removes 1 transition.');
    expect(card).not.toHaveTextContent('0 s');
    await userEvent.click(within(card).getByRole('button', { name: /^Apply/ }));
    await waitFor(() => expect(fake.applied).toEqual(['p1']));
    expect(useProject.getState().snapshot!.history.revision).toBe(4);
    // The Undo says what Apply did, as the card's own entry does, and takes it back through chat.
    const chip = await screen.findByRole('button', { name: 'Undo change speed. A transition is removed.' });
    await userEvent.click(chip);
    expect(fake.undone).toEqual([4]);
    expect(
      await screen.findByRole('button', { name: 'Redo change speed. A transition is removed.' }),
    ).toBeInTheDocument();
  });

  it('a speed that shortens a transition applies at once, says so, and its Undo names it', async () => {
    const undo = speedNote(0, 1);
    const fake = chat({
      reply: (r) =>
        reply(r.id, 'Sped up the middle part. A transition is shortened.', {
          applied: { change: changed(4, undo), undo },
        }),
      undoLast: (revision) => changed(revision + 1, null, { canRedo: true, redoLabel: 'chat.setClip.note' }),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'speed up the middle part 1.5x{Enter}');
    expect(await screen.findByText('Sped up the middle part. A transition is shortened.')).toBeInTheDocument();
    expect(screen.queryByRole('group', { name: /transition/ })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Undo change speed. A transition is shortened.' }));
    expect(fake.undone).toEqual([4]);
  });
});
