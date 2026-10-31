import { act, fireEvent, screen } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useAi } from '../../ai/aiStatus';
import { installChatBridge } from '../../bridge/chat';
import { fakeChat, proposal, reply } from '../../bridge/chatFake';
import { useChat } from '../../state/chat';
import { useChatPanel } from '../../state/chatPanel';
import { useProject } from '../../state/project';
import { useProposal } from '../../state/proposal';
import { useUi } from '../../state/ui';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { ChatPanel } from './ChatPanel';

beforeEach(() => {
  mockIPC(() => null);
  installChatBridge(fakeChat({ reply: (r) => reply(r.id, 'Done.') }));
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useAi.setState({ status: { mode: 'key' } });
  useProposal.getState().show(null);
  useChat.getState().reset(fixtureSnapshot().project.id);
  useChatPanel.setState({ collapsed: false });
  useUi.setState({ chatFocus: 0 });
});
const composer = () => screen.getByRole('textbox', { name: 'Message' });
const body = () => document.getElementById('chat-body')!;

describe('chat folds to a strip', () => {
  it('folds to the strip and keeps the panel mounted, hidden and inert', async () => {
    renderWithProviders(<ChatPanel />);
    const fold = screen.getByRole('button', { name: 'Collapse chat' });
    expect(fold).toHaveAttribute('aria-expanded', 'true');
    expect(fold).toHaveAttribute('aria-controls', 'chat-body');
    await userEvent.click(fold);
    expect(useChatPanel.getState().collapsed).toBe(true);
    const open = screen.getByRole('button', { name: 'Open chat' });
    expect(open).toHaveAttribute('aria-expanded', 'false');
    expect(open).toHaveAttribute('aria-controls', 'chat-body');
    expect(body()).toHaveAttribute('hidden');
    expect(body()).toHaveAttribute('inert');
    expect(body().querySelector('textarea')).not.toBeNull();
    expect(screen.getByRole('complementary', { name: 'Chat' })).toContainElement(open);
  });

  it('focus goes to the strip when chat folds with focus inside, and to the composer when it opens', async () => {
    renderWithProviders(<ChatPanel />);
    await userEvent.click(composer());
    act(() => useChatPanel.getState().toggle());
    expect(screen.getByRole('button', { name: 'Open chat' })).toHaveFocus();
    await userEvent.click(screen.getByRole('button', { name: 'Open chat' }));
    expect(useChatPanel.getState().collapsed).toBe(false);
    expect(composer()).toHaveFocus();
  });

  it('a fold while Klipolo is in the background still puts the keyboard on the strip', async () => {
    renderWithProviders(<ChatPanel />);
    await userEvent.click(composer());
    // Command Tab away: the box loses the window's focus but stays the focused element.
    fireEvent.blur(composer());
    await act(() => Promise.resolve());
    act(() => useChatPanel.getState().setCollapsed(true));
    expect(screen.getByRole('button', { name: 'Open chat' })).toHaveFocus();
  });

  it('Command K (focusChat) opens a folded chat with the cursor in the composer', () => {
    useChatPanel.setState({ collapsed: true });
    renderWithProviders(<ChatPanel />);
    act(() => useUi.getState().focusChat());
    expect(useChatPanel.getState().collapsed).toBe(false);
    expect(composer()).toHaveFocus();
  });

  it('the strip opens a chat whose box is off with the keyboard on its body, never the page', async () => {
    useAi.setState({ status: { mode: 'none' } });
    useChatPanel.setState({ collapsed: true });
    renderWithProviders(<ChatPanel />);
    await userEvent.click(screen.getByRole('button', { name: 'Open chat' }));
    expect(composer()).toBeDisabled();
    expect(body()).toContainElement(document.activeElement as HTMLElement);
  });

  it('opening chat brings the newest reply into view', () => {
    const scroll = vi.spyOn(Element.prototype, 'scrollIntoView');
    useChatPanel.setState({ collapsed: true });
    renderWithProviders(<ChatPanel />);
    act(() =>
      useChat.setState({
        messages: [{ id: 'r1', from: 'klipolo', kind: 'reply', ...reply('r1', 'Done.'), proposal: null }] as never,
      }),
    );
    scroll.mockClear();
    act(() => useChatPanel.getState().setCollapsed(false));
    expect(scroll).toHaveBeenCalled();
    scroll.mockRestore();
  });

  it('a waiting proposal stays red on the timeline while chat is folded, and the strip says it waits', async () => {
    installChatBridge(fakeChat({ reply: (r) => reply(r.id, 'I found one.', { proposal: proposal('p1', 3, { title: 'Cut 1 pause', removedUs: 1_000_000, removals: { ...proposal('x', 0).removals, ranges: [{ start: 1_000_000, end: 2_000_000 }] } }) }) }));
    renderWithProviders(<ChatPanel />);
    await userEvent.type(composer(), 'cut the pause{Enter}');
    await screen.findByRole('group', { name: 'Cut 1 pause' });
    act(() => useChatPanel.getState().setCollapsed(true));
    expect(useProposal.getState().proposal?.id).toBe('p1');
    expect(screen.getByRole('button', { name: 'Chat, 1 proposal waiting' })).toBeInTheDocument();
  });

  it('a reply is announced while chat is folded, outside the hidden part', async () => {
    let answer!: (o: ReturnType<typeof reply>) => void;
    installChatBridge(fakeChat({ reply: () => new Promise((r) => (answer = r)) }));
    renderWithProviders(<ChatPanel />);
    await userEvent.type(composer(), 'cut the intro{Enter}');
    act(() => useChatPanel.getState().setCollapsed(true));
    await act(async () => answer(reply(useChat.getState().pending!.id, 'Cut the intro.')));
    const said = screen.getByText('Klipolo: Cut the intro.');
    expect(said.closest('[aria-live="polite"]')).not.toBeNull();
    expect(said.closest('[inert], [hidden]')).toBeNull();
  });

  it('a failure is announced while chat is folded, outside the hidden part', async () => {
    let answer!: (o: { type: 'failed'; kind: 'busy'; message: string }) => void;
    installChatBridge(fakeChat({ reply: () => new Promise((r) => (answer = r)) }));
    renderWithProviders(<ChatPanel />);
    await userEvent.type(composer(), 'cut the intro{Enter}');
    act(() => useChatPanel.getState().setCollapsed(true));
    await act(async () => answer({ type: 'failed', kind: 'busy', message: 'Meta’s servers are busy right now.' }));
    const said = screen.getByText('Klipolo: Meta’s servers are busy right now.');
    expect(said.closest('[aria-live]')).not.toBeNull();
    expect(said.closest('[inert], [hidden]')).toBeNull();
  });

  it('a message not sent yet stays in the box across a fold', async () => {
    renderWithProviders(<ChatPanel />);
    await userEvent.type(composer(), 'make the intro shorter');
    act(() => useChatPanel.getState().setCollapsed(true));
    act(() => useChatPanel.getState().setCollapsed(false));
    expect(composer()).toHaveValue('make the intro shorter');
  });
});
