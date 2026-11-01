import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { TestProviders } from '../../test/render';

/*
 * The effect commands: add, change, switch, move and remove an effect on a clip or a layer, each one undo step running
 * the same action as the Effects popover. Each names the block it acts on (else the selection) and, but for Add, the
 * effect by its id in that stack. What they can't do is said as a toast.
 */

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn(), redo: vi.fn() },
  mediaUrl: (p: string) => p,
}));
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { useToasts } = await import('../../state/toasts');
const { engineApply, fx, onClipA } = await import('../../test/inspector');
const { useCommands } = await import('../command-palette/useCommands');

const lastTx = () => vi.mocked(bridge.apply).mock.calls.at(-1)![0];
const toasts = () => useToasts.getState().toasts.map((t) => t.text);

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  useToasts.setState({ toasts: [] });
  vi.mocked(bridge.apply).mockImplementation(engineApply());
});

describe('the effect commands', () => {
  it('add, change, switch and remove effects by the stack’s ids, each one undo step', async () => {
    useSelection.setState({ selectedId: 'title' });
    const { result } = renderHook(() => useCommands(), { wrapper: TestProviders });
    const run = (id: string, options: object) => act(() => result.current.find((c) => c.id === id)!.run(options));
    expect(result.current.find((c) => c.id === 'effect-add')!.enabled).toBe(true);
    run('effect-add', { effect: 'klipolo.builtin/effect.blur' });
    await waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(lastTx()).toEqual({
      label: 'effect.add.undo',
      params: { effect: 'klipolo.builtin/effect.blur' },
      ops: [
        {
          type: 'addEffect',
          target: { type: 'item', id: 'title' },
          effect: { id: '', effect: 'klipolo.builtin/effect.blur', enabled: true, params: {} },
          index: null,
        },
      ],
    });
    // On a stack that has it: by its id, on the block the run names.
    act(() => void onClipA([fx('fx', 'blur')]));
    run('effect-set', { block: 'a', effectId: 'fx', params: { radius: 30 } });
    await waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(2));
    expect(lastTx()).toEqual({
      label: 'effect.change.undo',
      params: { effect: 'klipolo.builtin/effect.blur' },
      ops: [{ type: 'updateEffect', target: { type: 'clip', id: 'a' }, id: 'fx', patch: { params: { radius: 30 } } }],
    });
    // Already on: nothing to do. Removed by its id.
    run('effect-toggle', { block: 'a', effectId: 'fx', enabled: true });
    run('effect-remove', { block: 'a', effectId: 'fx' });
    await waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(3));
    expect(lastTx()).toEqual({
      label: 'effect.remove.undo',
      params: { effect: 'klipolo.builtin/effect.blur' },
      ops: [{ type: 'removeEffect', target: { type: 'clip', id: 'a' }, id: 'fx' }],
    });
    // "Make the camera round" is one step: the effect with the settings it starts with.
    run('effect-add', { block: 'a', effect: 'klipolo.builtin/effect.mask', params: { shape: 'circle' } });
    await waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(4));
    expect(lastTx().ops).toEqual([
      {
        type: 'addEffect',
        target: { type: 'clip', id: 'a' },
        effect: { id: '', effect: 'klipolo.builtin/effect.mask', enabled: true, params: { shape: 'circle' } },
        index: null,
      },
    ]);
  });

  it('moves an effect up and down, and lists each only where it can act on the focused row', async () => {
    onClipA([fx('x', 'blur'), fx('y', 'glow')]);
    useSelection.setState({ selectedId: 'a' });
    const { result } = renderHook(() => useCommands(), { wrapper: TestProviders });
    const command = (id: string) => result.current.find((c) => c.id === id)!;
    expect(command('effect-move-up').fits!({ block: 'a', effectId: 'x' })).toBe(false);
    expect(command('effect-move-down').fits!({ block: 'a', effectId: 'x' })).toBe(true);
    act(() => command('effect-move-down').run({ block: 'a', effectId: 'x' }));
    await waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    expect(lastTx().ops).toEqual([{ type: 'moveEffect', target: { type: 'clip', id: 'a' }, id: 'x', index: 1 }]);
    // Listed only while an effect row has focus; Add and Change need a type or settings a list can't give.
    for (const id of ['effect-toggle', 'effect-remove', 'effect-move-up', 'effect-move-down'])
      expect([command(id).needs, command(id).paletteHidden]).toEqual(['effect', undefined]);
    for (const id of ['effect-add', 'effect-set'])
      expect(command(id)).toMatchObject({ needs: 'effect', paletteHidden: true });
  });

  it('says why when there is no stack, no such effect, or no such kind of effect', async () => {
    useSelection.setState({ selectedId: 'song' });
    const { result } = renderHook(() => useCommands(), { wrapper: TestProviders });
    const run = (id: string, options: object) => act(() => result.current.find((c) => c.id === id)!.run(options));
    expect(result.current.find((c) => c.id === 'effect-add')!.enabled).toBe(false);
    run('effect-add', { effect: 'klipolo.builtin/effect.blur' });
    run('effect-remove', { block: 'a', effectId: 'gone' });
    run('effect-add', { block: 'a', effect: 'com.example/neon' });
    expect(toasts()).toEqual([
      'Select a clip, or a layer that draws, to change its effects.',
      "That effect isn't on this clip or layer.",
      "That effect isn't in this version of Klipolo.",
    ]);
    expect(bridge.apply).not.toHaveBeenCalled();
  });
});
