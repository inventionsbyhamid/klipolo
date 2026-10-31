/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it } from 'vitest';
import type { ChatOutcome } from '../../bindings/ChatOutcome';
import type { CodedError } from '../../bindings/CodedError';
import { useAi } from '../../ai/aiStatus';
import { useEditorShortcuts } from '../command-palette/useEditorShortcuts';
import { useAnalysis } from '../../state/analysis';
import { installChatBridge } from '../../bridge/chat';
import { fakeChat, proposal, reply, type FakeChatScript } from '../../bridge/chatFake';
import { useChat, useChatStandIn } from '../../state/chat';
import { beginCommit } from '../../state/commitQueue';
import { historySteps } from '../../state/project';
import { useToasts } from '../../state/toasts';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useProposal } from '../../state/proposal';
import { useRange } from '../../state/range';
import { useSelection } from '../../state/selection';
import { useSpeech } from '../../state/speech';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { ChatPanel } from './ChatPanel';

const S = 1_000_000;
const code = (c: string, params: CodedError['params'] = {}): CodedError => ({ code: c, params });
/** The undo label Rust gives a title card chat added. */
const titleCard = () => code('chat.makeTitleCard', { title: 'Step 3' });
let calls: Record<string, any[]> = {};

/** Rust's project side: edits and undo answer the next revision; `extra` answers other commands. */
function backend(extra: Record<string, (args: any) => unknown> = {}) {
  calls = {};
  mockIPC((cmd, args) => {
    (calls[cmd] ??= []).push(args);
    if (extra[cmd]) return extra[cmd](args);
    return null;
  });
}

/** The project after a change: the next revision with `undo` as its history's top. */
function changed(revision: number, undo: CodedError | null, extra: Partial<ReturnType<typeof fixtureSnapshot>['history']> = {}) {
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

/** A cut of the ums and pauses on the fixture (30 s): 2.4 s in three ranges, made on revision 3. */
const cuts = () =>
  proposal('p1', 3, {
    title: 'Cut 2 pauses and 1 filler word',
    removedUs: 2.4 * S,
    removals: {
      ...proposal('x', 0).removals,
      ranges: [
        { start: 2 * S, end: 2.4 * S },
        { start: 9 * S, end: 10 * S },
        { start: 21 * S, end: 22 * S },
      ],
    },
    tags: [
      { startUs: 2 * S, endUs: 2.4 * S, label: 'um' },
      { startUs: 9 * S, endUs: 10 * S, label: 'pause' },
      { startUs: 21 * S, endUs: 22 * S, label: 'pause' },
    ],
  });

function chat(script: Partial<FakeChatScript> = {}) {
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

describe('chat', () => {
  it('sends a message and shows the applied reply with an Undo chip that takes it back, then Redo', async () => {
    const undo = titleCard();
    const fake = chat({ reply: (r) => reply(r.id, 'Added the title.', { applied: { change: changed(4, undo), undo } }) });
    backend({ project_undo: () => changed(5, null, { canRedo: true, redoLabel: 'chat.makeTitleCard' }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a title Step 3{Enter}');
    expect(screen.getByText('add a title Step 3')).toBeInTheDocument();
    expect(await screen.findByText('Added the title.')).toBeInTheDocument();
    expect(fake.requests[0]).toMatchObject({ text: 'add a title Step 3', selection: null, playheadUs: 0, keepOnly: [] });
    // Merged: one change, one undo step.
    expect(useProject.getState().snapshot!.history.revision).toBe(4);
    await userEvent.click(screen.getByRole('button', { name: 'Undo add the title Step 3' }));
    await waitFor(() => expect(calls.project_undo).toHaveLength(1));
    // Taken back: the chip turns into Redo for a while.
    expect(await screen.findByRole('button', { name: 'Redo add the title Step 3' })).toBeInTheDocument();
    // A change after that buries it: the chip goes for good, never back as an Undo that does nothing.
    act(() =>
      useProject.setState({ snapshot: { ...useProject.getState().snapshot!, history: changed(6, code('clip.split')).history } }),
    );
    expect(screen.queryByRole('button', { name: /^(Undo|Redo) add the title/ })).toBeNull();
  });

  it('takes back nothing once another change came after it: the chip says so', async () => {
    const undo = titleCard();
    chat({ reply: (r) => reply(r.id, 'Added the title.', { applied: { change: changed(4, undo), undo } }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a title{Enter}');
    await screen.findByText('Added the title.');
    // A hand edit lands after it.
    act(() => useProject.setState({ snapshot: { ...useProject.getState().snapshot!, history: changed(5, code('clip.split')).history } }));
    const chip = screen.getByRole('button', { name: 'Undo add the title Step 3' });
    expect(chip).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(chip);
    expect(calls.project_undo).toBeUndefined();
  });

  it('shows what a proposal removes, red on the timeline, and applies it as one undo step with Enter on the card', async () => {
    const title = code('chat.removeFillers');
    const fake = chat({
      reply: (r) => reply(r.id, 'I found 2 pauses and 1 um.', { proposal: cuts() }),
      apply: () => ({ change: changed(4, title), undo: title }),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the ums and long pauses{Enter}');
    const card = await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    expect(card).toHaveTextContent('2.4 s in total. 0:30.0 now, 0:27.6 after.');
    expect(within(card).getByText('2.4 s')).toHaveClass('cut');
    // The same proposal on the timeline: its ranges and tags.
    expect(useProposal.getState().proposal).toEqual({
      id: 'p1',
      baseRevision: 3,
      removals: cuts().removals,
      tags: cuts().tags,
    });
    act(() => card.focus());
    fireEvent.keyDown(card, { key: 'Enter' });
    await waitFor(() => expect(fake.applied).toEqual(['p1']));
    expect(await screen.findByText(/^Applied\./)).toBeInTheDocument();
    // The applied row says the total once, after the title Rust gave it.
    expect(screen.getByText('Cut 2 pauses and 1 filler word, 2.4 s in total.', { exact: false })).toBeInTheDocument();
    expect(screen.queryByText(/in total.*in total/)).toBeNull();
    expect(useProject.getState().snapshot!.history.revision).toBe(4);
    expect(useProposal.getState().proposal).toBeNull();
    expect(screen.getByRole('button', { name: /^Undo/ })).toBeInTheDocument();
    expect(message()).toHaveFocus();
  });

  it('a format-only proposal names what goes, never "0 s"', async () => {
    const p = proposal('p2', 3, {
      title: 'Drop the landscape format',
      removals: { ...proposal('x', 0).removals, formats: ['landscape'] },
    });
    chat({ reply: (r) => reply(r.id, 'This drops the 16:9 version.', { proposal: p }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'just vertical{Enter}');
    const card = await screen.findByRole('group', { name: 'Drop the landscape format' });
    expect(card).toHaveTextContent('Drops the 16:9 version and what was placed for it.');
    expect(card).not.toHaveTextContent('0 s');
    expect(card).not.toHaveTextContent('in total');
  });

  it('Discard leaves the project as it is, and the card says so', async () => {
    const fake = chat({ reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the pauses{Enter}');
    await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    await userEvent.click(screen.getByRole('button', { name: 'Discard' }));
    expect(await screen.findByText('Discarded.')).toBeInTheDocument();
    expect(fake.discarded).toEqual(['p1']);
    expect(useProposal.getState().proposal).toBeNull();
    expect(calls.project_apply).toBeUndefined();
  });

  it('drops a proposal when the project is edited by hand while it is open, with a note', async () => {
    chat({ reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }) });
    backend({ project_apply: () => changed(4, code('clip.split')) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the pauses{Enter}');
    await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    await act(() => useProject.getState().apply('clip.split' as never, [{ type: 'split', atUs: 5 * S }]));
    expect(
      await screen.findByText('This proposal was dropped because the project changed. Ask again to get a new one.'),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Apply' })).toBeNull();
    expect(useProposal.getState().proposal).toBeNull();
  });

  it('drops a proposal Rust refuses because the project moved', async () => {
    chat({
      reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }),
      apply: () => {
        throw new Error('The video changed. Try again.', { cause: code('project.moved') });
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the pauses{Enter}');
    await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    await userEvent.click(screen.getByRole('button', { name: 'Apply' }));
    expect(
      await screen.findByText('This proposal was dropped because the project changed. Ask again to get a new one.'),
    ).toBeInTheDocument();
  });

  it('lists every cut: hovering one shows its exact range, clicking seeks', async () => {
    chat({ reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the pauses{Enter}');
    await userEvent.click(await screen.findByRole('button', { name: 'Show all 3 cuts' }));
    const pause = screen.getByRole('button', { name: /^pause at 0:09\.0/ });
    await userEvent.hover(pause);
    expect(await screen.findByRole('tooltip')).toHaveTextContent('0:09.0 to 0:10.0, 1.0 s');
    expect(useProposal.getState().hovered).toBe(1);
    await userEvent.click(pause);
    expect(usePlayback.getState().playheadUs).toBe(9 * S);
  });

  it('never takes the chat box: Enter there sends while a card waits, and applies nothing', async () => {
    const fake = chat({ reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the pauses{Enter}');
    await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    expect(message()).toHaveFocus();
    await userEvent.keyboard('and the intro{Enter}');
    await waitFor(() => expect(fake.requests).toHaveLength(2));
    expect(fake.applied).toEqual([]);
  });

  it('keeps a message typed while Klipolo answers, and says why', async () => {
    let answer!: (o: ReturnType<typeof reply>) => void;
    chat({ reply: () => new Promise((r) => (answer = r)) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the intro{Enter}');
    expect(await screen.findByText('Thinking')).toBeInTheDocument();
    await userEvent.type(message(), 'and the outro{Enter}');
    expect(message()).toHaveValue('and the outro');
    expect(screen.getByText('Klipolo is still answering. Send this once it is done.')).toBeInTheDocument();
    await act(async () => answer(reply(useChat.getState().pending!.id, 'Cut.')));
    expect(await screen.findByText('Cut.')).toBeInTheDocument();
    expect(screen.queryByText('Thinking')).toBeNull();
  });

  it('sends the selection as "this", and the chip clears it', async () => {
    const fake = chat();
    useRange.setState({ range: { startUs: 4 * S, endUs: 6.5 * S } });
    renderWithProviders(<ChatPanel />);
    expect(screen.getByText('Selection 0:04.0 to 0:06.5')).toBeInTheDocument();
    await userEvent.type(message(), 'cut this{Enter}');
    await waitFor(() => expect(fake.requests).toHaveLength(1));
    expect(fake.requests[0].selection).toEqual({ start: 4 * S, end: 6.5 * S });
    await userEvent.click(screen.getByRole('button', { name: 'Clear selection' }));
    expect(useRange.getState().range).toBeNull();
    expect(screen.queryByText(/^Selection /)).toBeNull();
  });

  it('names a selected title in the chip and sends it as "this"', async () => {
    const fake = chat();
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(<ChatPanel />);
    expect(screen.getByText(/^Title: /)).toBeInTheDocument();
    await userEvent.type(message(), 'make it bigger{Enter}');
    await waitFor(() => expect(fake.requests[0]?.item).toBe('title'));
  });

  it('moment chips seek and play; Keep only these sends their ranges', async () => {
    const fake = chat({
      reply: (r) =>
        reply(r.id, 'She talks about the kiln in 2 places.', {
          moments: [
            { startUs: 3 * S, endUs: 5 * S, label: 'The first firing' },
            { startUs: 21 * S, endUs: 24 * S, label: 'The cracked bowl' },
          ],
        }),
    });
    backend({ engine_play: () => null });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'find the kiln{Enter}');
    await userEvent.click(await screen.findByRole('button', { name: '0:21.0 The cracked bowl' }));
    expect(usePlayback.getState().playheadUs).toBe(21 * S);
    expect(usePlayback.getState().playing).toBe(true);
    await userEvent.click(screen.getByRole('button', { name: 'Keep only these' }));
    await waitFor(() => expect(fake.requests).toHaveLength(2));
    expect(fake.requests[1]).toMatchObject({
      text: 'Keep only these',
      keepOnly: [
        { start: 3 * S, end: 5 * S },
        { start: 21 * S, end: 24 * S },
      ],
    });
  });

  it('says what went wrong in plain words, with Retry, and the project untouched', async () => {
    let busy = true;
    const fake = chat({
      reply: (r) =>
        busy
          ? { type: 'failed', kind: 'busy', message: 'Klipolo AI is busy right now. Try again in a moment.' }
          : reply(r.id, 'Done.'),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the ums{Enter}');
    expect(await screen.findByText('Klipolo AI is busy right now. Try again in a moment.')).toBeInTheDocument();
    expect(calls.project_apply).toBeUndefined();
    busy = false;
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByText('Done.')).toBeInTheDocument();
    expect(fake.requests.map((r) => r.text)).toEqual(['cut the ums', 'cut the ums']);
    // The person's message shows once; the failure is gone.
    expect(screen.getAllByText('cut the ums')).toHaveLength(1);
    expect(screen.queryByText(/busy right now/)).toBeNull();
  });

  it('a message the AI declines says so and goes back into the box to reword, with no Retry', async () => {
    const fake = chat({
      reply: () => ({ type: 'failed', kind: 'declined', message: 'The AI turned this down. Try wording it differently.' }),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'blur the faces{Enter}');
    expect(await screen.findByText('The AI turned this down. Try wording it differently.')).toBeInTheDocument();
    // The same text would be declined again: no one-tap Retry, the text is back in the box.
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
    expect(message()).toHaveValue('blur the faces');
    await userEvent.clear(message());
    await userEvent.type(message(), 'make the faces hard to recognize{Enter}');
    await waitFor(() => expect(fake.requests).toHaveLength(2));
  });

  it('a budget refusal offers Settings', async () => {
    chat({
      reply: () => ({ type: 'failed', kind: 'budget', message: 'Your access code has $0.03 left, and this needs about $0.04.' }),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the ums{Enter}');
    expect(await screen.findByText('Your access code has $0.03 left, and this needs about $0.04.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Open Settings' }));
    expect(useAi.getState().settingsOpen).toBe(true);
    useAi.setState({ settingsOpen: false });
  });

  it('with an access code this Mac couldn’t register, says why with Retry, never asking for a key', async () => {
    useAi.setState({ status: { mode: 'relay', code: 'KLIP-TEST-0002' } });
    chat({
      reply: () => ({
        type: 'failed',
        kind: 'registration',
        message: 'This Mac couldn’t reach Klipolo to use your access code. Check your connection, then retry.',
      }),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the ums{Enter}');
    expect(
      await screen.findByText('This Mac couldn’t reach Klipolo to use your access code. Check your connection, then retry.'),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument();
    expect(screen.queryByText('Add your Meta API key or access code in Settings')).toBeNull();
  });

  it('without a key or a code: the NeedsKey card, greyed suggestions and a disabled box', () => {
    useAi.setState({ status: { mode: 'none' } });
    chat();
    renderWithProviders(<ChatPanel />);
    expect(screen.getByText('Add your Meta API key or access code in Settings')).toBeInTheDocument();
    expect(message()).toBeDisabled();
    expect(message()).toHaveAttribute('placeholder', 'Add a key or code to chat');
    expect(screen.queryByRole('button', { name: 'cut the ums and long pauses' })).toBeNull();
  });

  it('the box works only while the AI status allows it', () => {
    useAi.setState({ status: { mode: 'relay', code: 'KLIP-TEST-0002', active: false } });
    chat();
    renderWithProviders(<ChatPanel />);
    expect(message()).toBeDisabled();
    act(() => useAi.setState({ status: { mode: 'relay', code: 'KLIP-TEST-0002', active: true } }));
    expect(message()).toBeEnabled();
  });

  it('a suggestion sends itself', async () => {
    const fake = chat();
    renderWithProviders(<ChatPanel />);
    expect(screen.getByRole('heading', { name: 'Ask for an edit' })).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'cut the ums and long pauses' }));
    await waitFor(() => expect(fake.requests.map((r) => r.text)).toEqual(['cut the ums and long pauses']));
  });

  it('the keyboard goes back to the chat box when a card collapses (Escape, Enter)', async () => {
    const title = code('chat.removeFillers');
    chat({
      reply: (r) => reply(r.id, 'I found 3.', { proposal: { ...cuts(), id: `p-${r.id}` } }),
      apply: () => ({ change: changed(4, title), undo: title }),
    });
    renderWithProviders(<ChatPanel />);
    const card = () => screen.getByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    await userEvent.type(message(), 'cut the pauses{Enter}');
    await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    expect(message()).toHaveFocus();
    act(() => card().focus());
    await userEvent.keyboard('{Escape}');
    expect(await screen.findByText('Discarded.')).toBeInTheDocument();
    expect(message()).toHaveFocus();
    await userEvent.type(message(), 'cut them again{Enter}');
    await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    act(() => card().focus());
    await userEvent.keyboard('{Enter}');
    expect(await screen.findByText(/^Applied\./)).toBeInTheDocument();
    expect(message()).toHaveFocus();
  });

  it('New chat starts over, and Rust forgets the conversation', async () => {
    const fake = chat();
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'hello{Enter}');
    await screen.findByText('Done.');
    await userEvent.click(screen.getByRole('button', { name: 'New chat' }));
    expect(screen.queryByText('Done.')).toBeNull();
    expect(screen.getByRole('heading', { name: 'Ask for an edit' })).toBeInTheDocument();
    expect(fake.resets).toBe(1);
  });

  it('another project starts the conversation over, and its open proposal goes', async () => {
    chat({ reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the pauses{Enter}');
    await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    const other = fixtureSnapshot();
    other.project = { ...other.project, id: 'other' };
    act(() => useProject.setState({ snapshot: other }));
    expect(screen.queryByText('cut the pauses')).toBeNull();
    expect(useProposal.getState().proposal).toBeNull();
  });

  it('an edit made on a project that changed meanwhile takes the project as it is now', async () => {
    const now = { ...fixtureSnapshot(), history: { ...fixtureSnapshot().history, revision: 9 } };
    backend({
      project_apply: () => {
        throw code('project.moved');
      },
      project_snapshot: () => now,
    });
    const applied = await useProject.getState().apply('text.add', [{ type: 'removeItem', id: 'title' }]);
    expect(applied).toBe(false);
    expect(calls.project_apply[0]).toMatchObject({ baseRevision: 3, projectId: 'project' });
    expect(useProject.getState().snapshot!.history.revision).toBe(9);
  });
});

describe('chat: one proposal at a time, moments of their revision, and the keys', () => {
  it('a newer proposal replaces the open one: one card stays open, the old one says so and Rust forgets it', async () => {
    let n = 0;
    const fake = chat({
      reply: (r) => {
        n += 1;
        const title = n === 1 ? 'Cut 3 pauses' : 'Cut 3 parts';
        return reply(r.id, `Proposal ${n}.`, { proposal: proposal(`p${n}`, 3, { ...cuts(), id: `p${n}`, title }) });
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the pauses{Enter}');
    await screen.findByRole('group', { name: 'Cut 3 pauses' });
    await userEvent.type(message(), 'cut more{Enter}');
    await screen.findByRole('group', { name: 'Cut 3 parts' });
    expect(screen.queryByRole('group', { name: 'Cut 3 pauses' })).toBeNull();
    expect(screen.getAllByRole('button', { name: 'Apply' })).toHaveLength(1);
    expect(screen.getByText('Replaced by the newer proposal below.')).toBeInTheDocument();
    expect(fake.discarded).toEqual(['p1']);
    // Red is the open one's cuts only.
    expect(useProposal.getState().proposal?.id).toBe('p2');
  });

  it('sends the revision each request was made on; moments go stale once the project changes', async () => {
    /** The project moves on to `revision` (a hand edit, an undo). */
    const moveTo = (revision: number) =>
      act(() =>
        useProject.setState({
          snapshot: { ...useProject.getState().snapshot!, history: changed(revision, code('clip.split')).history },
        }),
      );
    const fake = chat({
      reply: (r) =>
        reply(r.id, 'Two moments.', {
          moments: [
            { startUs: 3 * S, endUs: 5 * S, label: 'The first firing' },
            { startUs: 21 * S, endUs: 24 * S, label: 'The cracked bowl' },
          ],
        }),
    });
    backend({ engine_play: () => null });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'find the kiln{Enter}');
    await screen.findByRole('button', { name: '0:21.0 The cracked bowl' });
    expect(fake.requests[0].baseRevision).toBe(3);
    expect(screen.getByRole('button', { name: 'Keep only these' })).toBeEnabled();
    moveTo(4);
    // The times are the old revision's: Keep only these waits for a new answer, and says why.
    expect(screen.getByRole('button', { name: 'Keep only these' })).toBeDisabled();
    expect(screen.getByText('The video changed since. Ask again to keep only these.')).toBeInTheDocument();
    // An undo back is a change too: they stay stale.
    moveTo(5);
    expect(screen.getByRole('button', { name: 'Keep only these' })).toBeDisabled();
    // The chips still seek.
    await userEvent.click(screen.getByRole('button', { name: '0:21.0 The cracked bowl' }));
    expect(usePlayback.getState().playheadUs).toBe(21 * S);
    await userEvent.type(message(), 'again{Enter}');
    await waitFor(() => expect(fake.requests).toHaveLength(2));
    expect(fake.requests[1].baseRevision).toBe(5);
  });

  it('Enter while an input method composes a word confirms the word and sends nothing', () => {
    const fake = chat();
    renderWithProviders(<ChatPanel />);
    fireEvent.change(message(), { target: { value: 'にほん' } });
    fireEvent.keyDown(message(), { key: 'Enter', keyCode: 229 });
    fireEvent.keyDown(message(), { key: 'Enter', isComposing: true });
    expect(fake.requests).toHaveLength(0);
    expect(message()).toHaveValue('にほん');
  });

  it('Retry does nothing while chat can’t send, and keeps the failure', async () => {
    const fake = chat({ reply: () => ({ type: 'failed', kind: 'network', message: 'No connection.' }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the ums{Enter}');
    await screen.findByText('No connection.');
    act(() => useAi.setState({ status: { mode: 'none' } }));
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(fake.requests).toHaveLength(1);
    expect(screen.getByText('No connection.')).toBeInTheDocument();
  });

  it('an Apply refused for another reason keeps the card open with why, and Retry applies it', async () => {
    let refuse = true;
    const undo = code('chat.removeFillers');
    chat({
      reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }),
      apply: () => {
        if (refuse) throw new Error('Klipolo couldn’t make that change.');
        return { change: changed(4, undo), undo };
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the pauses{Enter}');
    const card = await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    await userEvent.click(within(card).getByRole('button', { name: 'Apply' }));
    expect(await within(card).findByText('Klipolo couldn’t make that change.')).toBeInTheDocument();
    expect(within(card).getByRole('button', { name: 'Discard' })).toBeInTheDocument();
    expect(useProposal.getState().proposal?.id).toBe('p1');
    refuse = false;
    await userEvent.click(within(card).getByRole('button', { name: 'Retry' }));
    expect(await screen.findByText(/^Applied\./)).toBeInTheDocument();
  });

  it('names every kind of removal that isn’t footage', async () => {
    chat({
      reply: (r) =>
        reply(r.id, 'Here.', {
          proposal: proposal('p1', 3, {
            title: 'Apply these changes',
            removals: {
              ...proposal('x', 0).removals,
              tracks: ['t1', 't2'],
              luts: ['l1'],
              recordings: ['r1'],
              markers: ['m1', 'm2', 'm3'],
              sounds: ['r2'],
            },
          }),
        }),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'tidy up{Enter}');
    const card = await screen.findByRole('group', { name: 'Apply these changes' });
    for (const line of [
      'Removes 2 tracks.',
      'Takes 1 color look out of the project.',
      'Removes 1 recording.',
      'Removes or moves 3 markers.',
      'Leaves 1 recording with less of its sound.',
    ])
      expect(within(card).getByText(line)).toBeInTheDocument();
    expect(within(card).queryByText(/0\.0 s/)).toBeNull();
  });

  it('Command Enter applies the open proposal through the editor’s keys, but not while a dialog is open', async () => {
    /** The editor's keys, as the app mounts them beside chat. */
    function EditorKeys() {
      useEditorShortcuts();
      return null;
    }
    const fake = chat({ reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }) });
    renderWithProviders(
      <>
        <ChatPanel />
        <EditorKeys />
        <div role="dialog" aria-label="Settings">
          <button type="button">Close</button>
        </div>
      </>,
    );
    await userEvent.type(message(), 'cut the pauses{Enter}');
    await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    screen.getByRole('button', { name: 'Close' }).focus();
    await userEvent.keyboard('{Meta>}{Enter}{/Meta}');
    expect(fake.applied).toEqual([]);
    message().focus();
    await userEvent.keyboard('{Meta>}{Enter}{/Meta}');
    await waitFor(() => expect(fake.applied).toEqual(['p1']));
  });

  it('an empty chat says when the clips aren’t analysed yet, and while they are', () => {
    useAnalysis.setState({ states: {} });
    renderWithProviders(<ChatPanel />);
    expect(
      screen.getByText('Analyze the clips first to cut pauses, find moments and add captions. Format and music work now.'),
    ).toBeInTheDocument();
    act(() => useAnalysis.setState({ states: { screen: { state: 'running', step: 'words', pct: 40 } } }));
    expect(
      screen.getByText('Klipolo is still listening to these clips. Cuts and moments work when analysis is done.'),
    ).toBeInTheDocument();
    const done = { state: 'done', approximate: false } as const;
    act(() => useAnalysis.setState({ states: { screen: done, camera: done, mic: done, music: done, broll: done } }));
    expect(screen.queryByText(/Analyze the clips first|still listening/)).toBeNull();
  });

  it('the development stand-in lets chat run without a key written into the AI status', () => {
    useAi.setState({ status: { mode: 'none' } });
    renderWithProviders(<ChatPanel />);
    expect(message()).toBeDisabled();
    act(() => useChatStandIn.setState({ on: true }));
    expect(message()).toBeEnabled();
    expect(useAi.getState().status).toEqual({ mode: 'none' });
  });

  it('a Retry keeps the revision it was made on; a keep-only refused for a moved project says so, with no Retry', async () => {
    let n = 0;
    const fake = chat({
      reply: (r) => {
        n += 1;
        if (n === 1)
          return reply(r.id, 'Two moments.', {
            moments: [
              { startUs: 3 * S, endUs: 5 * S, label: 'The first firing' },
              { startUs: 21 * S, endUs: 24 * S, label: 'The cracked bowl' },
            ],
          });
        if (n === 2) return { type: 'failed', kind: 'network', message: 'No connection.' };
        throw new Error('The video changed. Try again.', { cause: code('project.moved') });
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'find the kiln{Enter}');
    await userEvent.click(await screen.findByRole('button', { name: 'Keep only these' }));
    await screen.findByText('No connection.');
    // The project moves on before Retry: the request still says the revision its moments' times are on.
    act(() =>
      useProject.setState({
        snapshot: { ...useProject.getState().snapshot!, history: changed(4, code('clip.split')).history },
      }),
    );
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(fake.requests).toHaveLength(3));
    expect(fake.requests[2]).toMatchObject({ baseRevision: 3, keepOnly: [{ start: 3 * S, end: 5 * S }, { start: 21 * S, end: 24 * S }] });
    expect(await screen.findByText('The video changed. Ask again.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
  });

  it('an Apply Rust made that the page couldn’t show closes the card with why, no red and no Retry', async () => {
    const undo = code('chat.removeFillers');
    chat({
      reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }),
      // Made on a revision this copy doesn't have: the page asks for the whole project, which fails.
      apply: () => ({ change: { ...changed(4, undo), baseRevision: 99 }, undo }),
    });
    backend({
      project_snapshot: () => {
        throw new Error('The project file can’t be read.');
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the pauses{Enter}');
    const card = await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    await userEvent.click(within(card).getByRole('button', { name: 'Apply' }));
    expect(
      await screen.findByText('Applied, but the timeline didn’t update. Reopen the project.'),
    ).toBeInTheDocument();
    expect(screen.queryByRole('group', { name: 'Cut 2 pauses and 1 filler word' })).toBeNull();
    expect(screen.queryByRole('button', { name: /^(Retry|Apply)$/ })).toBeNull();
    expect(useProposal.getState().proposal).toBeNull();
  });

  it('Keep only these sends the revision its reply says it landed on', async () => {
    let n = 0;
    const fake = chat({
      reply: (r) => {
        n += 1;
        return n === 1
          ? reply(r.id, 'Two moments.', {
              revision: 7,
              moments: [
                { startUs: 3 * S, endUs: 5 * S, label: 'The first firing' },
                { startUs: 21 * S, endUs: 24 * S, label: 'The cracked bowl' },
              ],
            })
          : reply(r.id, 'Done.');
      },
    });
    // The page is on that revision too (a reply on another one has stale moments).
    const at7 = fixtureSnapshot();
    useProject.setState({ snapshot: { ...at7, history: { ...at7.history, revision: 7 } } });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'find the kiln{Enter}');
    await userEvent.click(await screen.findByRole('button', { name: 'Keep only these' }));
    await waitFor(() => expect(fake.requests).toHaveLength(2));
    expect(fake.requests[1].baseRevision).toBe(7);
  });

  it('Undo takes a chat change back through chat with the revision it made, and says so plainly once it can’t', async () => {
    const undo = titleCard();
    let refuse = false;
    let made = 3;
    const fake = chat({
      reply: (r) => {
        made += 1;
        return reply(r.id, 'Added the title.', { applied: { change: changed(made, undo), undo } });
      },
      undoLast: (revision) => {
        if (refuse) throw new Error('The video changed.', { cause: code('project.moved') });
        return changed(revision + 1, null, { canRedo: true, redoLabel: 'chat.makeTitleCard' });
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a title{Enter}');
    await screen.findByText('Added the title.');
    refuse = true;
    await userEvent.click(screen.getByRole('button', { name: 'Undo add the title Step 3' }));
    expect(fake.undone).toEqual([4]);
    expect(
      await screen.findByText('This change can’t be undone from chat any more. Use Undo in the top bar.', {
        selector: 'p.muted',
      }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Undo add the title|Retry/ })).toBeNull();
    refuse = false;
    act(() => useChat.getState().newChat());
    await userEvent.type(message(), 'add a title{Enter}');
    await userEvent.click(await screen.findByRole('button', { name: 'Undo add the title Step 3' }));
    expect(fake.undone).toEqual([4, 5]);
    expect(await screen.findByRole('button', { name: 'Redo add the title Step 3' })).toBeInTheDocument();
  });

  it('the Undo chip waits its turn behind a change on its way, then takes back its own', async () => {
    const undo = titleCard();
    const fake = chat({
      reply: (r) => reply(r.id, 'Added the title.', { applied: { change: changed(4, undo), undo } }),
      undoLast: (revision) => changed(revision + 1, null, { canRedo: true, redoLabel: 'chat.makeTitleCard' }),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a title{Enter}');
    await screen.findByText('Added the title.');
    // A commit on its way (a drop, a slider let go): the chip's undo goes after it.
    const end = beginCommit();
    await userEvent.click(screen.getByRole('button', { name: 'Undo add the title Step 3' }));
    await new Promise((r) => setTimeout(r, 20));
    expect(fake.undone).toEqual([]);
    act(() => end());
    await waitFor(() => expect(fake.undone).toEqual([4]));
    expect(await screen.findByRole('button', { name: 'Redo add the title Step 3' })).toBeInTheDocument();
    expect(useProject.getState().snapshot!.history.revision).toBe(5);
  });

  it('the Undo chip says other refusals in a toast, and stays', async () => {
    const undo = titleCard();
    chat({
      reply: (r) => reply(r.id, 'Added the title.', { applied: { change: changed(4, undo), undo } }),
      undoLast: () => {
        throw new Error("Klipolo couldn't save this change. Try again.");
      },
    });
    useToasts.setState({ toasts: [] });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a title{Enter}');
    await screen.findByText('Added the title.');
    await userEvent.click(screen.getByRole('button', { name: 'Undo add the title Step 3' }));
    await waitFor(() =>
      expect(useToasts.getState().toasts.map((t) => t.text)).toContain("Klipolo couldn't save this change. Try again."),
    );
    expect(screen.getByRole('button', { name: 'Undo add the title Step 3' })).toBeInTheDocument();
    expect(screen.queryByText('This change can’t be undone from chat any more. Use Undo in the top bar.')).toBeNull();
    expect(useProject.getState().snapshot!.history.revision).toBe(4);
  });

  it('moments a reply found on another revision than the page has come stale', async () => {
    const moments = [{ startUs: 2 * S, endUs: 6 * S, label: 'the kiln' }];
    // The page is at 3; the reply landed on 2 (the project moved under it before the page heard).
    chat({ reply: (r) => reply(r.id, 'Here it is.', { moments, revision: 2 }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'find the kiln{Enter}');
    await screen.findByText('Here it is.');
    expect(screen.getByRole('button', { name: 'Keep only these' })).toBeDisabled();
    expect(screen.getByText('The video changed since. Ask again to keep only these.')).toBeInTheDocument();
  });

  it('a message whose Retry is refused because the video changed goes back into the box; Keep only does not', async () => {
    let n = 0;
    chat({
      reply: () => {
        n += 1;
        if (n === 1) return { type: 'failed', kind: 'busy', message: 'Klipolo AI is busy right now. Try again in a moment.' };
        throw new Error('The video changed. Try again.', { cause: code('project.moved') });
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut this{Enter}');
    await userEvent.click(await screen.findByRole('button', { name: 'Retry' }));
    expect(await screen.findByText('The video changed. Ask again.')).toBeInTheDocument();
    expect(message()).toHaveValue('cut this');
    // Keep only's words are a label, not something typed: they don't go into the box.
    act(() => useChat.getState().setDraft(''));
    await act(() => useChat.getState().send('Keep only these', { keepOnly: [{ start: 0, end: S }], baseRevision: 1 }));
    expect(message()).toHaveValue('');
  });

  it('a refused Undo chip is said aloud', async () => {
    const undo = titleCard();
    chat({
      reply: (r) => reply(r.id, 'Added the title.', { applied: { change: changed(4, undo), undo } }),
      undoLast: () => {
        throw new Error('The video changed.', { cause: code('project.moved') });
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a title{Enter}');
    await screen.findByText('Added the title.');
    await userEvent.click(screen.getByRole('button', { name: 'Undo add the title Step 3' }));
    const live = document.querySelector('[data-chat-notices]');
    await waitFor(() =>
      expect(live).toHaveTextContent('This change can’t be undone from chat any more. Use Undo in the top bar.'),
    );
  });

  it('a reply that lands after New chat still brings its change to the timeline', async () => {
    const undo = code('chat.setCaptions', { on: 'on' });
    let answer: (o: ChatOutcome) => void = () => undefined;
    chat({ reply: () => new Promise<ChatOutcome>((done) => (answer = done)) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add captions{Enter}');
    act(() => useChat.getState().newChat());
    await act(async () => answer(reply('m', 'Captions are on.', { applied: { change: changed(4, undo), undo } })));
    // Rust made the change: the timeline has it, though the conversation it belonged to is gone.
    await waitFor(() => expect(useProject.getState().snapshot!.history.revision).toBe(4));
    expect(screen.queryByText('Captions are on.')).toBeNull();
  });

  it('the Undo chip names the change chat made', async () => {
    const undo = code('chat.andMore', { first: code('chat.setFormat', { format: 'portrait', others: 'kept' }), more: 1 });
    chat({ reply: (r) => reply(r.id, 'Done.', { applied: { change: changed(4, undo), undo } }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'vertical with karaoke captions{Enter}');
    expect(await screen.findByRole('button', { name: 'Undo make it vertical, and 1 more' })).toBeInTheDocument();
  });

  it('Apply waits behind a change still on its way, and is no undo step; the chip’s undo is one', async () => {
    const title = code('chat.removeFillers');
    const fake = chat({
      reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }),
      apply: () => ({ change: changed(4, title), undo: title }),
      undoLast: (revision) => changed(revision + 1, null, { canRedo: true, redoLabel: 'chat.removeFillers' }),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the pauses{Enter}');
    await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    const steps = historySteps();
    const end = beginCommit();
    await userEvent.click(screen.getByRole('button', { name: 'Apply' }));
    await new Promise((r) => setTimeout(r, 20));
    expect(fake.applied).toEqual([]);
    act(() => end());
    await waitFor(() => expect(fake.applied).toEqual(['p1']));
    await waitFor(() => expect(useProject.getState().snapshot!.history.revision).toBe(4));
    expect(historySteps()).toBe(steps);
    // Its Undo chip is a real undo.
    await userEvent.click(await screen.findByRole('button', { name: 'Undo cut the filler words' }));
    await waitFor(() => expect(fake.undone).toEqual([4]));
    expect(historySteps()).toBe(steps + 1);
  });

  it('an Apply that lands after New chat still brings its change to the timeline', async () => {
    const title = code('chat.removeFillers');
    let land: () => void = () => undefined;
    chat({
      reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }),
      apply: async () => {
        await new Promise<void>((done) => (land = done));
        return { change: changed(4, title), undo: title };
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the pauses{Enter}');
    await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    await userEvent.click(screen.getByRole('button', { name: 'Apply' }));
    act(() => useChat.getState().newChat());
    act(() => land());
    await waitFor(() => expect(useProject.getState().snapshot!.history.revision).toBe(4));
  });
});

describe('chat: more on proposals and the panel', () => {
  it('drops an open proposal when the words are timed again, though the revision is the same', async () => {
    const fake = chat({ reply: (r) => reply(r.id, 'I found 3.', { proposal: cuts() }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'cut the ums{Enter}');
    await screen.findByRole('group', { name: 'Cut 2 pauses and 1 filler word' });
    // Captions' "Time the words again" finished: Rust says the speech changed.
    act(() => useSpeech.setState((s) => ({ pagesEpoch: s.pagesEpoch + 1 })));
    expect(
      await screen.findByText('This proposal was dropped because the project changed. Ask again to get a new one.'),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Apply' })).toBeNull();
    expect(useProposal.getState().proposal).toBeNull();
    expect(fake.applied).toEqual([]);
  });
});
