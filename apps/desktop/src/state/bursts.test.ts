import { act, renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { fixturePatch, fixtureSnapshot } from '../test/fixtures';

vi.mock('../bridge/commands', () => ({
  bridge: { apply: vi.fn(), closeProject: vi.fn() },
  mediaUrl: (p: string) => p,
}));
vi.mock('../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
const { bridge } = await import('../bridge/commands');
const { useProject } = await import('./project');
const { beginCommit } = await import('./commitQueue');
const { useBurst } = await import('./useBurst');
const { registerBurst } = await import('./bursts');

it('closing commits a change still settling in a burst, ahead of the close', async () => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  const order: string[] = [];
  vi.mocked(bridge.apply).mockImplementation((tx) => {
    order.push(JSON.stringify(tx).includes('#112233') ? 'apply #112233' : 'apply');
    return Promise.resolve(fixturePatch(fixtureSnapshot()));
  });
  vi.mocked(bridge.closeProject).mockImplementation(() => {
    order.push('close');
    return Promise.resolve();
  });
  // As a slider's arrow keys do: the change settles into one undo step, and closing commits it at once.
  const burst = renderHook(() => useBurst()).result;
  // Another commit is on its way, so the settling change waits its turn: kept at close, the close waits for it, and
  // it goes in once that commit has landed.
  const landed = beginCommit();
  act(() => burst.current.push({ label: 'text.color.undo', ops: [{ type: 'rename', name: '#112233' }] }));
  let closed!: Promise<void>;
  act(() => void (closed = useProject.getState().close()));
  landed();
  await act(() => closed);
  expect(order).toEqual(['apply #112233', 'close']);
});

it("closing goes round again for a change one burst hands to another that the round already passed", async () => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  const order: string[] = [];
  vi.mocked(bridge.apply).mockImplementation((tx) => {
    order.push(JSON.stringify(tx).includes('#445566') ? 'apply #445566' : 'apply');
    return Promise.resolve(fixturePatch(fixtureSnapshot()));
  });
  vi.mocked(bridge.closeProject).mockImplementation(() => {
    order.push('close');
    return Promise.resolve();
  });
  // A text style menu's burst, registered as it mounts, then its colour field's settle, registered after it: flushed,
  // the settle hands its colour to the menu's burst, which this round has already passed. Only a second round sends it.
  const menu = renderHook(() => useBurst()).result;
  let colour: string | null = '#445566';
  const off = registerBurst(() => {
    if (colour === null) return false;
    menu.current.push({ label: 'text.color.undo', ops: [{ type: 'rename', name: colour }] });
    colour = null;
    return true;
  });
  let closed!: Promise<void>;
  act(() => void (closed = useProject.getState().close()));
  await act(() => closed);
  off();
  // The second round committed it, ahead of the close.
  expect(order).toEqual(['apply #445566', 'close']);
});
