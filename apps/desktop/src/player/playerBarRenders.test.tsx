import { act } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/** The bar's icon buttons, counted as they draw. */
const drawn = vi.hoisted(() => ({ n: 0 }));
vi.mock('../design/IconButton', async (original) => {
  const real = await original<typeof import('../design/IconButton')>();
  return {
    ...real,
    IconButton: (props: Parameters<typeof real.IconButton>[0]) => {
      drawn.n += 1;
      return <real.IconButton {...props} />;
    },
  };
});
vi.mock('../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../bridge/engine', () => ({
  engine: { seek: vi.fn(() => Promise.resolve()), play: vi.fn(() => Promise.resolve()), pause: vi.fn(() => Promise.resolve(0)) },
}));
const { useProject } = await import('../state/project');
const { usePlayback } = await import('../state/playback');
const { useChat } = await import('../state/chat');
const { PlayerBar } = await import('./PlayerBar');

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  usePlayback.setState({ playheadUs: 0, playing: false, rate: 1 });
});

describe('the player bar', () => {
  it("draws its buttons again for none of the changes that leave them as they are: an edit, playing, chat's words", () => {
    renderWithProviders(<PlayerBar />);
    expect(drawn.n).toBeGreaterThan(0);
    drawn.n = 0;
    act(() => {
      const s = useProject.getState().snapshot!;
      useProject.setState({ snapshot: { ...s, project: { ...s.project, name: 'Renamed' } } });
    });
    act(() => usePlayback.setState({ playing: true }));
    act(() => usePlayback.setState({ playing: false }));
    act(() => useChat.setState({ messages: [{ id: 'm', from: 'person', text: 'cut the ums' }] }));
    expect(drawn.n).toBe(0);
  });
});
