/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, screen, waitFor, within } from '@testing-library/react';
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
import { beginCommit } from '../../state/commitQueue';
import { historySteps } from '../../state/project';
import { useProject } from '../../state/project';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { ChatPanel } from './ChatPanel';

const code = (c: string, params: CodedError['params'] = {}): CodedError => ({ code: c, params });
/** The undo label Rust gives a placed sticker. */
const placedSticker = () => code('chat.makeSticker', { prompt: 'paper plane' });
const BUSY = 'Muse Image is busy right now, so try again in a moment.';
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

const plane: ChatPicture = {
  id: 'pic1',
  kind: 'sticker',
  prompt: 'paper plane',
  place: 'Goes in the middle, over the moment it was asked for.',
};

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
/** Where the panel says what isn't a message (a picture ready, placed). */
const notices = () => document.querySelector('[data-chat-notices]');
/** The card of the paper plane, once the reply is there. */
const card = () => screen.findByRole('group', { name: 'Paper plane sticker' });
const art = (c: HTMLElement) => c.querySelector('img');

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
    const c = await card();
    await waitFor(() => expect(art(c)).not.toBeNull());
    expect(fake.drawn).toEqual(['pic1']);
    expect(art(c)).toHaveAttribute('src', expect.stringContaining('paper-plane-1.png'));
    // The title and the group name say what it is; the picture adds nothing for a screen reader to repeat.
    expect(art(c)).toHaveAttribute('alt', '');
    expect(c).toHaveTextContent(
      '1600 x 1600, transparent background. Goes in the middle, over the moment it was asked for.',
    );
    expect(within(c).getByRole('button', { name: 'Use' })).toBeInTheDocument();
    expect(within(c).getByRole('button', { name: 'Try again' })).toBeInTheDocument();
    await waitFor(() => expect(notices()).toHaveTextContent('Paper plane sticker is ready.'));
    expect(useProject.getState().snapshot!.history.revision).toBe(3);
  });

  it('says what each drawing costs while it draws, the price Rust counts for a picture', async () => {
    let release: () => void = () => undefined;
    chat({
      drawPicture: async () => {
        await new Promise<void>((done) => (release = done));
        return picture(1);
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    expect(await within(c).findByText('Drawing it now.')).toBeInTheDocument();
    expect(within(c).getByText('Costs about $0.01.')).toBeInTheDocument();
    await act(async () => release());
    await waitFor(() => expect(within(c).queryByText('Costs about $0.01.')).toBeNull());
    const pricing = readFileSync(resolve(__dirname, '../../../../../crates/klipolo-meta/src/pricing.rs'), 'utf8');
    expect(pricing).toMatch(/pub const IMAGE_USD: f64 = 0\.01;/);
  });

  it('Use places it as one undo step: the card shows its Undo chip, with the keyboard on it', async () => {
    const undo = placedSticker();
    const fake = chat({ drawPicture: () => picture(1), usePicture: () => ({ change: changed(4, undo), undo }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    await userEvent.click(await within(c).findByRole('button', { name: 'Use' }));
    expect(fake.used).toEqual(['pic1']);
    await waitFor(() => expect(useProject.getState().snapshot!.history.revision).toBe(4));
    const chip = await within(c).findByRole('button', { name: 'Undo add the paper plane sticker' });
    expect(chip).toHaveFocus();
    expect(within(c).queryByRole('button', { name: 'Use' })).toBeNull();
    await waitFor(() => expect(notices()).toHaveTextContent('Placed.'));
  });

  it('a placed picture the timeline couldn’t take in says so', async () => {
    const undo = placedSticker();
    // A change on a revision the page doesn't have, and the project can't be read again.
    backend({
      project_snapshot: () => {
        throw "Klipolo couldn't read the project.";
      },
    });
    chat({ drawPicture: () => picture(1), usePicture: () => ({ change: changed(9, undo), undo }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    await userEvent.click(await within(c).findByRole('button', { name: 'Use' }));
    const said = 'Placed, but the timeline didn’t update. Reopen the project.';
    expect(await within(c).findByText(said)).toBeInTheDocument();
    expect(within(c).queryByRole('button')).toBeNull();
  });

  it('Try again keeps the picture and Use while it draws another, and draws once however often pressed', async () => {
    let n = 0;
    let release: () => void = () => undefined;
    const fake = chat({
      drawPicture: async () => {
        n += 1;
        if (n === 2) await new Promise<void>((done) => (release = done));
        return picture(n);
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    await userEvent.click(await within(c).findByRole('button', { name: 'Try again' }));
    expect(await within(c).findByText('Drawing a new one.')).toBeInTheDocument();
    expect(art(c)).toHaveAttribute('src', expect.stringContaining('paper-plane-1.png'));
    expect(within(c).getByRole('button', { name: 'Use' })).toBeInTheDocument();
    expect(c.querySelector('.spinner')).not.toBeNull();
    // Pressed again while it draws: no second drawing, no second payment.
    await act(() => useChat.getState().drawPicture(useChat.getState().messages[1].id, 'pic1'));
    expect(fake.drawn).toEqual(['pic1', 'pic1']);
    act(() => release());
    await waitFor(() => expect(art(c)).toHaveAttribute('src', expect.stringContaining('paper-plane-2.png')));
    expect(c.querySelector('.spinner')).toBeNull();
  });

  it('a Try again that fails keeps the picture drawn before, and Use places it', async () => {
    let n = 0;
    const undo = placedSticker();
    const fake = chat({
      drawPicture: () => {
        n += 1;
        if (n === 2) throw new Error('Muse Image is busy right now, so try again in a moment.');
        return picture(n);
      },
      usePicture: () => ({ change: changed(4, undo), undo }),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    await userEvent.click(await within(c).findByRole('button', { name: 'Try again' }));
    expect(await within(c).findByRole('alert')).toHaveTextContent(BUSY);
    expect(art(c)).toHaveAttribute('src', expect.stringContaining('paper-plane-1.png'));
    await userEvent.click(within(c).getByRole('button', { name: 'Use' }));
    expect(fake.used).toEqual(['pic1']);
    expect(await within(c).findByRole('button', { name: 'Undo add the paper plane sticker' })).toBeInTheDocument();
  });

  it('a first drawing that fails says why, with a still placeholder and Try again', async () => {
    let busy = true;
    chat({
      drawPicture: () => {
        if (busy) throw new Error('Muse Image is busy right now, so try again in a moment.');
        return picture(1);
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    expect(await within(c).findByRole('alert')).toHaveTextContent(BUSY);
    expect(c.querySelector('.spinner')).toBeNull();
    expect(within(c).queryByRole('button', { name: 'Use' })).toBeNull();
    busy = false;
    await userEvent.click(within(c).getByRole('button', { name: 'Try again' }));
    expect(await within(c).findByRole('button', { name: 'Use' })).toBeInTheDocument();
  });

  it('a sticker whose moment was cut says so, with the keyboard on why, and offers nothing more', async () => {
    const cut =
      "The part of the video that sticker was for was cut while it was drawn, so I didn't place it. " +
      'Ask me for it again.';
    chat({
      drawPicture: () => picture(1),
      usePicture: () => {
        throw new Error(cut, { cause: cut });
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    await userEvent.click(await within(c).findByRole('button', { name: 'Use' }));
    const why = await within(c).findByRole('alert');
    expect(why).toHaveTextContent(cut);
    expect(why).toHaveFocus();
    expect(within(c).queryByRole('button')).toBeNull();
    expect(useProject.getState().snapshot!.history.revision).toBe(3);
  });

  it('a title card background keeps its name, from what it shows', async () => {
    const background: ChatPicture = {
      id: 'pic2',
      kind: 'titleBackground',
      prompt: 'a calm sea',
      place: 'Goes behind the title, for as long as it shows.',
    };
    chat({ reply: (r) => reply(r.id, 'Drawing the background.', { pictures: [background] }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'a title card over a calm sea{Enter}');
    const c = await screen.findByRole('group', { name: 'A calm sea title card background' });
    expect(await within(c).findByRole('alert')).toBeInTheDocument();
    expect(c).toHaveTextContent('Goes behind the title, for as long as it shows.');
  });

  it("drives the app's chat commands with what each names", async () => {
    const undo = code('chat.makeSticker');
    backend({
      chat_draw_picture: () => picture(1),
      chat_use_picture: () => ({ change: changed(4, undo), undo }),
      chat_undo_last: () => changed(5, undo),
    });
    expect(await tauriChat.drawPicture('pic1')).toEqual(picture(1));
    await tauriChat.usePicture('pic1', picture(1).path);
    await tauriChat.undoLast(4);
    expect(calls.chat_draw_picture).toEqual([{ id: 'pic1' }]);
    expect(calls.chat_use_picture).toEqual([{ id: 'pic1', path: picture(1).path }]);
    expect(calls.chat_undo_last).toEqual([{ revision: 4 }]);
  });

  it('Use during Try again places the picture shown, and the card stops saying it draws', async () => {
    let n = 0;
    let release: () => void = () => undefined;
    const undo = placedSticker();
    const fake = chat({
      drawPicture: async () => {
        n += 1;
        if (n === 2) await new Promise<void>((done) => (release = done));
        return picture(n);
      },
      usePicture: () => ({ change: changed(4, undo), undo }),
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    await userEvent.click(await within(c).findByRole('button', { name: 'Try again' }));
    await within(c).findByText('Drawing a new one.');
    await userEvent.click(within(c).getByRole('button', { name: 'Use' }));
    // Rust is told which drawing: the one the card shows.
    expect(fake.usedPaths).toEqual([picture(1).path]);
    await within(c).findByRole('button', { name: 'Undo add the paper plane sticker' });
    act(() => release());
    await waitFor(() => expect(fake.drawn).toEqual(['pic1', 'pic1']));
    await act(async () => undefined);
    expect(within(c).queryByText('Drawing a new one.')).toBeNull();
    expect(c.querySelector('.spinner')).toBeNull();
    expect(c).not.toHaveAttribute('aria-busy');
  });

  it('a Use refused during Try again keeps Use and says a new one is still drawing, until it lands', async () => {
    let n = 0;
    let land: () => void = () => undefined;
    chat({
      drawPicture: async () => {
        n += 1;
        if (n === 2) await new Promise<void>((resolve) => (land = resolve));
        return picture(n);
      },
      usePicture: () => {
        throw new Error('The video changed. Try again.', { cause: code('project.moved') });
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    await userEvent.click(await within(c).findByRole('button', { name: 'Try again' }));
    await userEvent.click(within(c).getByRole('button', { name: 'Use' }));
    expect(await within(c).findByRole('alert')).toHaveTextContent('The video changed. Try again.');
    expect(within(c).getByRole('button', { name: 'Use' })).toBeInTheDocument();
    // The second drawing is still on its way: the card says so instead of offering a Try again that can't start.
    expect(within(c).getByText('Drawing a new one.')).toBeInTheDocument();
    expect(within(c).queryByRole('button', { name: 'Try again' })).toBeNull();
    land();
    expect(await within(c).findByRole('button', { name: 'Try again' })).toBeInTheDocument();
    expect(within(c).queryByText('Drawing a new one.')).toBeNull();
    expect(c).not.toHaveAttribute('aria-busy');
  });

  it('Use waits behind a change still on its way, and is no undo step', async () => {
    const undo = placedSticker();
    const fake = chat({ drawPicture: () => picture(1), usePicture: () => ({ change: changed(4, undo), undo }) });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    const steps = historySteps();
    const end = beginCommit();
    await userEvent.click(await within(c).findByRole('button', { name: 'Use' }));
    await new Promise((r) => setTimeout(r, 20));
    expect(fake.used).toEqual([]);
    act(() => end());
    await waitFor(() => expect(fake.used).toEqual(['pic1']));
    await waitFor(() => expect(useProject.getState().snapshot!.history.revision).toBe(4));
    // A sticker placed is no undo or redo for the caption words' press queue.
    expect(historySteps()).toBe(steps);
  });

  it('a Use that lands after New chat still brings its change to the timeline', async () => {
    const undo = placedSticker();
    let land: () => void = () => undefined;
    chat({
      drawPicture: () => picture(1),
      usePicture: async () => {
        await new Promise<void>((done) => (land = done));
        return { change: changed(4, undo), undo };
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    await userEvent.click(await within(c).findByRole('button', { name: 'Use' }));
    act(() => useChat.getState().newChat());
    act(() => land());
    await waitFor(() => expect(useProject.getState().snapshot!.history.revision).toBe(4));
  });

  it('a notice said while Klipolo thinks is still heard', async () => {
    let answer: (o: ReturnType<typeof reply>) => void = () => undefined;
    let n = 0;
    chat({
      reply: (r) => {
        n += 1;
        if (n === 1) return reply(r.id, 'Made one with Muse Image.', { pictures: [plane] });
        return new Promise((done) => (answer = done));
      },
      drawPicture: async () => {
        await new Promise((r) => setTimeout(r, 50));
        return picture(1);
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    await card();
    // Another message thinks while the picture lands.
    await userEvent.type(message(), 'and make it vertical{Enter}');
    await waitFor(() => expect(notices()).toHaveTextContent('Paper plane sticker is ready.'));
    act(() => answer(reply('m2', 'Done.')));
    await screen.findByText('Done.');
    expect(notices()).toHaveTextContent('Paper plane sticker is ready.');
  });

  it('the card stops looking after the keyboard once the person went elsewhere', async () => {
    let n = 0;
    let release: () => void = () => undefined;
    chat({
      drawPicture: async () => {
        n += 1;
        if (n === 2) await new Promise<void>((done) => (release = done));
        return picture(n);
      },
    });
    renderWithProviders(<ChatPanel />);
    await userEvent.type(message(), 'add a paper plane sticker{Enter}');
    const c = await card();
    // Try again: its button goes while it draws, and the keyboard goes to the card.
    await userEvent.click(await within(c).findByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(c).toHaveFocus());
    // The person clicks somewhere that takes no focus: the keyboard is on the page now.
    act(() => c.blur());
    await act(async () => undefined);
    act(() => release());
    await waitFor(() => expect(art(c)).toHaveAttribute('src', expect.stringContaining('paper-plane-2.png')));
    expect(c).not.toHaveFocus();
    expect(document.activeElement).toBe(document.body);
  });
});
