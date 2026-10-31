/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { mockConvertFileSrc, mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it } from 'vitest';
import type { ChatPicture } from '../../bindings/ChatPicture';
import type { ChatPictureDrawn } from '../../bindings/ChatPictureDrawn';
import type { CodedError } from '../../bindings/CodedError';
import { useAi } from '../../ai/aiStatus';
import { installChatBridge, tauriChat } from '../../bridge/chat';
import { fakeChat, reply, type FakeChatScript } from '../../bridge/chatFake';
import { useAnalysis } from '../../state/analysis';
import { useChat, useChatStandIn } from '../../state/chat';
import { useProject } from '../../state/project';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { ChatPanel } from './ChatPanel';

const code = (c: string): CodedError => ({ code: c, params: {} });
let calls: Record<string, any[]> = {};

function backend(extra: Record<string, (args: any) => unknown> = {}) {
  calls = {};
  mockIPC((cmd, args) => {
    (calls[cmd] ??= []).push(args);
    if (extra[cmd]) return extra[cmd](args);
    return null;
  });
  mockConvertFileSrc('macos');
}

/** The project after a change: the next revision with `undo` as its history's top. */
function changed(revision: number, undo: CodedError) {
  const snapshot = fixtureSnapshot();
  return {
    ...fixturePatch(snapshot),
    baseRevision: revision - 1,
    history: { ...snapshot.history, revision, canUndo: true, undoLabel: undo.code, undoParams: undo.params },
  };
}

const plane: ChatPicture = { id: 'pic1', kind: 'sticker', prompt: 'paper plane' };

function picture(n: number): ChatPictureDrawn {
  return {
    id: 'pic1',
    path: `/Movies/Talk Media/Muse Image/paper-plane-${n}.png`,
    name: 'Paper plane sticker',
    width: 1600,
    height: 1600,
    madeWith: 'Muse Image',
  };
}

function chat(script: Partial<FakeChatScript> = {}) {
  const fake = fakeChat({ reply: (r) => reply(r.id, 'Made one with Muse Image.', { pictures: [plane] }), ...script });
  installChatBridge(fake);
  return fake;
}

const message = () => screen.getByRole('textbox', { name: 'Message' });

beforeEach(() => {
  backend();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useAi.setState({ status: { mode: 'key', keyHint: '6789' } });
  useChat.getState().reset(fixtureSnapshot().project.id);
  useChatStandIn.setState({ on: false });
  const done = { state: 'done', approximate: false } as const;
  useAnalysis.setState({ states: { screen: done, camera: done, mic: done, music: done, broll: done } });
});

describe('pictures chat asks for', () => {
  it('draws the picture a reply asks for and shows it with Use and Try again', async () => {
    const fake = chat({ drawPicture: () => picture(1) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const card = await screen.findByRole('group', { name: 'Paper plane sticker' });
    await waitFor(() => expect(within(card).getByRole('img', { name: 'Paper plane sticker' })).toBeInTheDocument());
    expect(fake.drawn).toEqual(['pic1']);
    expect(within(card).getByRole('img')).toHaveAttribute('src', expect.stringContaining('paper-plane-1.png'));
    expect(card).toHaveTextContent('Made with Muse Image.');
    expect(within(card).getByRole('button', { name: 'Use' })).toBeInTheDocument();
    expect(within(card).getByRole('button', { name: 'Try again' })).toBeInTheDocument();
    expect(useProject.getState().snapshot!.history.revision).toBe(3);
  });

  it('Use places it as one undo step, and the card shows an Undo chip', async () => {
    const undo = code('chat.makeSticker');
    const fake = chat({ drawPicture: () => picture(1), usePicture: () => ({ change: changed(4, undo), undo }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const card = await screen.findByRole('group', { name: 'Paper plane sticker' });
    await userEvent.click(await within(card).findByRole('button', { name: 'Use' }));
    expect(fake.used).toEqual(['pic1']);
    await waitFor(() => expect(useProject.getState().snapshot!.history.revision).toBe(4));
    expect(await within(card).findByRole('button', { name: /^Undo/ })).toBeInTheDocument();
    expect(within(card).queryByRole('button', { name: 'Use' })).toBeNull();
  });

  it('Try again draws it again', async () => {
    let n = 0;
    const fake = chat({ drawPicture: () => picture(++n) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const card = await screen.findByRole('group', { name: 'Paper plane sticker' });
    await userEvent.click(await within(card).findByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(within(card).getByRole('img')).toHaveAttribute('src', expect.stringContaining('paper-plane-2.png')));
    expect(fake.drawn).toEqual(['pic1', 'pic1']);
  });

  it('a drawing that fails says why, with Try again', async () => {
    let busy = true;
    chat({
      drawPicture: () => {
        if (busy) throw new Error('Muse Image is busy right now, so try again in a moment.');
        return picture(1);
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const card = await screen.findByRole('group', { name: 'Paper plane sticker' });
    expect(await within(card).findByText('Muse Image is busy right now, so try again in a moment.')).toBeInTheDocument();
    expect(within(card).queryByRole('button', { name: 'Use' })).toBeNull();
    busy = false;
    await userEvent.click(within(card).getByRole('button', { name: 'Try again' }));
    expect(await within(card).findByRole('button', { name: 'Use' })).toBeInTheDocument();
  });

  it('a sticker whose moment was cut says so, and offers nothing more', async () => {
    const cut = "The part of the video that sticker was for was cut while it was drawn, so I didn't place it.";
    chat({
      drawPicture: () => picture(1),
      usePicture: () => {
        throw new Error(cut, { cause: cut });
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const card = await screen.findByRole('group', { name: 'Paper plane sticker' });
    await userEvent.click(await within(card).findByRole('button', { name: 'Use' }));
    expect(await within(card).findByText(cut)).toBeInTheDocument();
    expect(within(card).queryByRole('button')).toBeNull();
    expect(useProject.getState().snapshot!.history.revision).toBe(3);
  });

  it("drives the app's chat commands with what each names", async () => {
    const undo = code('chat.makeSticker');
    backend({
      chat_draw_picture: () => picture(1),
      chat_use_picture: () => ({ change: changed(4, undo), undo }),
      chat_undo_last: () => changed(5, undo),
    });
    expect(await tauriChat.drawPicture('pic1')).toEqual(picture(1));
    await tauriChat.usePicture('pic1');
    await tauriChat.undoLast(4);
    expect(calls.chat_draw_picture).toEqual([{ id: 'pic1' }]);
    expect(calls.chat_use_picture).toEqual([{ id: 'pic1' }]);
    expect(calls.chat_undo_last).toEqual([{ revision: 4 }]);
  });
});
