import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { useAi } = await import('../../ai/aiStatus');
const { installChatBridge } = await import('../../bridge/chat');
const { fakeChat, proposal, reply } = await import('../../bridge/chatFake');
const { useChat } = await import('../../state/chat');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useProposal } = await import('../../state/proposal');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('../timeline/Timeline');
const { ChatPanel } = await import('./ChatPanel');

const S = 1_000_000;

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useProposal.getState().show(null);
  usePlayback.setState({ playheadUs: 0, playing: false });
  useUi.setState({ pxPerSecond: 40 });
  useAi.setState({ status: { mode: 'key' } });
  useChat.getState().reset(fixtureSnapshot().project.id);
});

describe('chat and the timeline', () => {
  it('proposal_ranges_reach_the_timeline: the card and the red cuts show one proposal, and go together', async () => {
    const p = proposal('p1', 3, {
      title: 'Cut 1 pause and 1 um',
      removedUs: 2 * S,
      removals: {
        ...proposal('x', 0).removals,
        ranges: [
          { start: 2 * S, end: 2.5 * S },
          { start: 12 * S, end: 13.5 * S },
        ],
      },
      tags: [
        { startUs: 2 * S, endUs: 2.5 * S, label: 'um' },
        { startUs: 12 * S, endUs: 13.5 * S, label: 'pause' },
      ],
    });
    installChatBridge(fakeChat({ reply: (r) => reply(r.id, 'I found 2.', { proposal: p }) }));
    renderWithProviders(
      <>
        <Timeline />
        <ChatPanel />
      </>,
    );
    expect(document.querySelectorAll('[data-cut]')).toHaveLength(0);
    await userEvent.type(screen.getByRole('textbox', { name: 'Message' }), 'cut the um and the pause{Enter}');
    await screen.findByRole('group', { name: 'Cut 1 pause and 1 um' });
    const cuts = () => [...document.querySelectorAll<HTMLElement>('[data-cut]')];
    expect(cuts().map((c) => c.textContent)).toEqual(['um', 'pause']);
    // Pointing at a cut in the card's list lights it on the timeline.
    await userEvent.click(screen.getByRole('button', { name: 'Show all 2 cuts' }));
    await userEvent.hover(screen.getByRole('button', { name: /^pause at 0:12\.0/ }));
    expect(cuts()[1]).toHaveClass('is-hovered');
    expect(cuts()[0]).not.toHaveClass('is-hovered');
    // Discarded: the card says so, and the timeline has no red left.
    await userEvent.click(screen.getByRole('button', { name: 'Discard' }));
    await waitFor(() => expect(cuts()).toHaveLength(0));
    expect(screen.getByText('Discarded.')).toBeInTheDocument();
    act(() => useProposal.getState().hover(null));
  });
});
