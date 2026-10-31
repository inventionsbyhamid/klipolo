import { beforeEach, describe, expect, it, vi } from 'vitest';

/*
 * What the preview engine tells the page once (a notice): said in a toast, in words that name the file and what to do.
 */

vi.mock('../../bridge/commands', () => ({ bridge: {}, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({ engine: { seek: vi.fn(() => Promise.resolve()) } }));
const { useToasts } = await import('../../state/toasts');
const { onEngineEvent } = await import('./useEngineSession');

beforeEach(() => useToasts.setState({ toasts: [] }));

describe('the preview engine’s notices', () => {
  it('names a file that isn’t where it was, and says how to get it back', () => {
    onEngineEvent({ type: 'notice', notice: { code: 'copies.failed.missing', params: { name: 'intro.mov' } } });
    expect(useToasts.getState().toasts.map((t) => [t.kind, t.text])).toEqual([
      ['error', "intro.mov isn't where it was. Put it back, then open the project again."],
    ]);
  });
});
