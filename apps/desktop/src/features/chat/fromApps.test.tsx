/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { emit } from '@tauri-apps/api/event';
import { mockConvertFileSrc, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { ChatPicture } from '../../bindings/ChatPicture';
import type { ChatReply } from '../../bindings/ChatReply';
import type { CodedError } from '../../bindings/CodedError';
import type { ExternalReply } from '../../bindings/ExternalReply';
import { useAi } from '../../ai/aiStatus';
import { installChatBridge } from '../../bridge/chat';
import { fakeChat, proposal, type FakeChat } from '../../bridge/chatFake';
import { useAnalysis } from '../../state/analysis';
import { followApps, useChat, useChatStandIn } from '../../state/chat';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useProposal } from '../../state/proposal';
import { useRange } from '../../state/range';
import { useSelection } from '../../state/selection';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { ChatPanel } from './ChatPanel';

const S = 1_000_000;
const code = (c: string, params: CodedError['params'] = {}): CodedError => ({ code: c, params });
/** The undo label Rust gives a format change another app made: the compiler's, with the app's name. */
const vertical = () => code('chat.setFormat', { format: 'portrait', client: 'Example Client' });
let calls: Record<string, any[]> = {};
let stop: (() => void) | null = null;

/** Rust's side, its events mocked: `extra` answers commands; the rest answer nothing. */
function backend(extra: Record<string, (args: any) => unknown> = {}) {
  calls = {};
  mockIPC(
    (cmd, args) => {
      (calls[cmd] ??= []).push(args);
      if (extra[cmd]) return extra[cmd](args);
      return null;
    },
    { shouldMockEvents: true },
  );
  mockConvertFileSrc('macos');
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

/** The opening of the project the tests have open. */
const OPENING = 1;
/** The fixture's project, as this opening of it. */
const opened = () => ({ ...fixtureSnapshot(), opening: OPENING });

/** What another app did, as Rust tells it, about this opening: an id of its own, no words, and `parts`. */
function external(client: string, parts: Partial<ChatReply>): ExternalReply {
  return {
    client,
    opening: OPENING,
    reply: {
      id: crypto.randomUUID(),
      text: '',
      notes: [],
      questions: [],
      applied: null,
      undone: null,
      proposal: null,
      moments: [],
      revision: fixtureSnapshot().history.revision,
      pictures: [],
      ...parts,
    },
  };
}

/** Rust tells the page what an app did (`mcp-reply`). */
async function tell(reply: ExternalReply) {
  await act(async () => {
    await emit('mcp-reply', reply);
  });
}

/** A keep-only proposal of 30 s the app made on revision 3. */
const teaser = () =>
  proposal('p1', 3, {
    title: 'Keep only the suggested parts',
    removedUs: 30 * S,
    removals: { ...proposal('x', 0).removals, ranges: [{ start: 0, end: 30 * S }] },
  });

function chat(script: Partial<Parameters<typeof fakeChat>[0]> = {}): FakeChat {
  const fake = fakeChat({ reply: () => Promise.reject(new Error('not asked')), ...script });
  installChatBridge(fake);
  return fake;
}

beforeEach(async () => {
  backend();
  useProject.setState({ snapshot: opened(), pending: 0, switching: false });
  useSelection.setState({ selectedId: null });
  useRange.setState({ range: null });
  useProposal.getState().show(null);
  usePlayback.setState({ playheadUs: 0, playing: false });
  useAi.setState({ status: { mode: 'key', keyHint: '6789' } });
  useChat.getState().reset(fixtureSnapshot().project.id);
  useChatStandIn.setState({ on: false });
  const done = { state: 'done', approximate: false } as const;
  useAnalysis.setState({ states: { screen: done, camera: done, mic: done, music: done, broll: done } });
  stop = await followApps();
});

afterEach(() => {
  stop?.();
  stop = null;
});

describe('another app in chat', () => {
  it('external_reply_shows_from_client_and_adopts_the_patch', async () => {
    chat();
    renderWithProviders(<ChatPanel />);
    const undo = vertical();
    await tell(external('Example Client', { applied: { change: changed(4, undo), undo }, revision: 4 }));
    // Merged as one change, the app's.
    await waitFor(() => expect(useProject.getState().snapshot!.history.revision).toBe(4));
    expect(screen.getByText('From Example Client')).toBeInTheDocument();
    // What it did, in the history's words, under the app's name.
    expect(screen.getByText('Make it vertical')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Undo make it vertical (from Example Client)' })).toBeInTheDocument();
  });

  it('an_apps_undo_chip_takes_its_change_back_with_the_persons_undo', async () => {
    const fake = chat();
    backend({ project_undo: () => changed(5, null, { canRedo: true, redoLabel: 'chat.setFormat', redoParams: vertical().params }) });
    stop?.();
    stop = await followApps();
    renderWithProviders(<ChatPanel />);
    const undo = vertical();
    await tell(external('Example Client', { applied: { change: changed(4, undo), undo }, revision: 4 }));
    await userEvent.click(await screen.findByRole('button', { name: 'Undo make it vertical (from Example Client)' }));
    // Chat's own Undo is only for chat's last change; an app's step is taken back as the top bar's Undo does.
    await waitFor(() => expect(calls.project_undo).toHaveLength(1));
    expect(fake.undone).toEqual([]);
    expect(await screen.findByRole('button', { name: 'Redo make it vertical (from Example Client)' })).toBeInTheDocument();
  });

  it('external_proposal_sets_useProposal_and_names_the_client', async () => {
    const applied = changed(4, code('chat.keepOnly'));
    const fake = chat({ apply: () => ({ change: applied, undo: code('chat.keepOnly') }) });
    renderWithProviders(<ChatPanel />);
    await tell(external('Muse Spark agent', { proposal: teaser() }));
    // Red on the timeline, as chat's own proposal is, and a card that says who suggested it.
    await waitFor(() => expect(useProposal.getState().proposal?.id).toBe('p1'));
    const card = screen.getByRole('group', { name: 'Keep only the suggested parts' });
    expect(within(card).getByText('Suggested by Muse Spark agent')).toBeInTheDocument();
    expect(screen.getByText('From Muse Spark agent')).toBeInTheDocument();
    // Nothing is cut until the person's Apply, which goes to Rust by the proposal's id.
    expect(useProject.getState().snapshot!.history.revision).toBe(3);
    await userEvent.click(within(card).getByRole('button', { name: 'Apply' }));
    await waitFor(() => expect(fake.applied).toEqual(['p1']));
    expect(await screen.findByText('Applied.')).toBeInTheDocument();
    expect(useProject.getState().snapshot!.history.revision).toBe(4);
    expect(useProposal.getState().proposal).toBeNull();
  });

  it('an_apps_change_then_its_proposal_come_in_order', async () => {
    chat();
    renderWithProviders(<ChatPanel />);
    const undo = vertical();
    // One call: the additions applied, then the removals proposed on the project they leave.
    await act(async () => {
      void emit('mcp-reply', external('Example Client', { applied: { change: changed(4, undo), undo }, revision: 4 }));
      void emit('mcp-reply', external('Example Client', { proposal: { ...teaser(), baseRevision: 4 }, revision: 4 }));
    });
    await waitFor(() => expect(useProposal.getState().proposal?.id).toBe('p1'));
    expect(screen.getAllByText('From Example Client')).toHaveLength(2);
    expect(screen.getByRole('button', { name: 'Apply' })).toBeInTheDocument();
  });

  it('a_newer_proposal_from_an_app_replaces_the_one_waiting', async () => {
    const fake = chat();
    renderWithProviders(<ChatPanel />);
    await tell(external('Example Client', { proposal: teaser() }));
    await tell(external('Example Client', { proposal: { ...teaser(), id: 'p2' } }));
    await waitFor(() => expect(useProposal.getState().proposal?.id).toBe('p2'));
    expect(screen.getByText('Replaced by the newer proposal below.')).toBeInTheDocument();
    // Rust replaced it already; telling it to forget the old one changes nothing there.
    expect(fake.discarded).toEqual(['p1']);
  });

  it('an_apps_moments_show_as_chats_do', async () => {
    chat();
    renderWithProviders(<ChatPanel />);
    await tell(external('Example Client', { moments: [{ startUs: 5 * S, endUs: 9 * S, label: 'The big reveal' }] }));
    expect(await within(screen.getByRole('log')).findByText('From Example Client')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '0:05.0 The big reveal' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Keep only these' })).toBeEnabled();
  });

  it('an_apps_picture_is_drawn_and_placed_as_its_call_said', async () => {
    const plane: ChatPicture = { id: 'pic1', kind: 'sticker', prompt: 'paper plane', place: 'Goes in the middle.' };
    const sticker = code('chat.makeSticker', { prompt: 'paper plane' });
    const fake = chat({
      drawPicture: () => ({
        id: 'pic1',
        path: '/Movies/Talk Media/Muse Image/paper-plane-1.png',
        name: 'Paper plane sticker',
        width: 1600,
        height: 1600,
        madeWith: 'Muse Image',
      }),
      usePicture: () => ({ change: changed(4, sticker), undo: sticker }),
    });
    renderWithProviders(<ChatPanel />);
    await tell(external('Example Client', { pictures: [plane] }));
    // Drawn as chat's are (nothing else draws it), then placed: the app's call said it appears once ready.
    await waitFor(() => expect(fake.used).toEqual(['pic1']));
    expect(fake.drawn).toEqual(['pic1']);
    expect(fake.usedPaths).toEqual(['/Movies/Talk Media/Muse Image/paper-plane-1.png']);
    await waitFor(() => expect(useProject.getState().snapshot!.history.revision).toBe(4));
    expect(await screen.findByRole('button', { name: 'Undo add the paper plane sticker' })).toBeInTheDocument();
  });

  it('a_reply_about_another_opening_is_ignored', async () => {
    chat();
    useProject.setState({ snapshot: { ...fixtureSnapshot(), opening: 2 } });
    renderWithProviders(<ChatPanel />);
    const undo = vertical();
    // Made on the project's opening before this one (revisions start again at 0 on each): not this project's change.
    const before = { ...external('Example Client', { applied: { change: changed(4, undo), undo }, revision: 4 }), opening: 1 };
    await tell(before);
    expect(useChat.getState().messages).toEqual([]);
    expect(useProject.getState().snapshot!.history.revision).toBe(3);
    // This opening's is taken.
    await tell({ ...before, opening: 2 });
    await waitFor(() => expect(useProject.getState().snapshot!.history.revision).toBe(4));
    expect(within(screen.getByRole('log')).getByText('From Example Client')).toBeInTheDocument();
  });

  it('nothing_shows_with_no_project_open', async () => {
    chat();
    renderWithProviders(<ChatPanel />);
    act(() => useProject.setState({ snapshot: null }));
    await tell(external('Example Client', { moments: [{ startUs: 0, endUs: S, label: 'Intro' }] }));
    expect(useChat.getState().messages).toEqual([]);
  });
});
