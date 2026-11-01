import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';

/*
 * The effect actions: each one undo step through the commit queue, built from the stack as the project has it when it
 * goes in, so a quick second action builds on the first and an effect gone by then sends nothing.
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
const { engineApply, fx, onClipA, withLayers } = await import('../../test/inspector');
const ops = await import('./effectOps');

const lastTx = () => vi.mocked(bridge.apply).mock.calls.at(-1)![0];
const clipA = () => useProject.getState().snapshot!.project.timeline.main[0];

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  vi.mocked(bridge.apply).mockImplementation(engineApply());
});
afterEach(() => vi.useRealTimers());

describe('the effect stack of a block', () => {
  it('a clip has one, and so has every layer that draws; a sound or a zoom has none', () => {
    withLayers(
      { id: 'zoom', content: { type: 'zoom', target: { type: 'auto' }, scale: 2 } },
      { id: 'adjust', content: { type: 'adjustment' } },
    );
    const project = useProject.getState().snapshot!.project;
    expect(ops.stackOf(project, 'a')).toEqual({ target: { type: 'clip', id: 'a' }, effects: [] });
    expect(ops.stackOf(project, 'title')?.target).toEqual({ type: 'item', id: 'title' });
    expect(ops.stackOf(project, 'adjust')?.target).toEqual({ type: 'item', id: 'adjust' });
    expect(ops.stackOf(project, 'song')).toBeNull();
    expect(ops.stackOf(project, 'zoom')).toBeNull();
    expect(ops.stackOf(project, 'nothing')).toBeNull();
  });
});

describe('the effect actions', () => {
  it('adds an effect at the end, with the settings it starts with, named by its id', async () => {
    const stack = { target: { type: 'clip' as const, id: 'a' }, effects: [] };
    await ops.addEffect(stack, 'klipolo.builtin/effect.mask', { shape: 'circle' });
    expect(lastTx()).toEqual({
      label: 'effect.add.undo',
      params: { effect: 'klipolo.builtin/effect.mask' },
      ops: [
        {
          type: 'addEffect',
          target: { type: 'clip', id: 'a' },
          effect: { id: '', effect: 'klipolo.builtin/effect.mask', enabled: true, params: { shape: 'circle' } },
          index: null,
        },
      ],
    });
  });

  it('a stack of eight takes no more', async () => {
    const eight = Array.from({ length: ops.MAX_EFFECTS }, (_, i) => fx(`fx${i}`, 'vignette'));
    onClipA(eight);
    await ops.addEffect({ target: { type: 'clip', id: 'a' }, effects: eight }, 'klipolo.builtin/effect.blur');
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it('moves one place up or down, and never past an end', async () => {
    onClipA([fx('x', 'blur'), fx('y', 'vignette')]);
    const stack = { target: { type: 'clip' as const, id: 'a' }, effects: clipA().effects };
    await ops.moveEffect(stack, stack.effects[0], true);
    expect(bridge.apply).not.toHaveBeenCalled();
    await ops.moveEffect(stack, stack.effects[0], false);
    expect(lastTx()).toEqual({
      label: 'effect.down.undo',
      params: { effect: 'klipolo.builtin/effect.blur' },
      ops: [{ type: 'moveEffect', target: { type: 'clip', id: 'a' }, id: 'x', index: 1 }],
    });
    expect(clipA().effects.map((e) => e.id)).toEqual(['y', 'x']);
  });

  it('a quick second move builds on the first, still on its way', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    vi.mocked(bridge.apply).mockImplementation(engineApply(50));
    onClipA([fx('x', 'blur'), fx('y', 'vignette'), fx('z', 'glow')]);
    const stack = { target: { type: 'clip' as const, id: 'a' }, effects: clipA().effects };
    const first = ops.moveEffect(stack, stack.effects[0], false);
    const second = ops.moveEffect(stack, stack.effects[0], false);
    await vi.advanceTimersByTimeAsync(200);
    await Promise.all([first, second]);
    expect(vi.mocked(bridge.apply).mock.calls.map(([tx]) => tx.ops[0])).toEqual([
      { type: 'moveEffect', target: { type: 'clip', id: 'a' }, id: 'x', index: 1 },
      { type: 'moveEffect', target: { type: 'clip', id: 'a' }, id: 'x', index: 2 },
    ]);
  });

  it('turns an effect off, and removes it; once gone, nothing is sent and remove says so', async () => {
    onClipA([fx('x', 'blur')]);
    const stack = { target: { type: 'clip' as const, id: 'a' }, effects: clipA().effects };
    const blur = stack.effects[0];
    await ops.turnEffect(stack, blur, false);
    expect(lastTx()).toEqual({
      label: 'effect.off.undo',
      params: { effect: 'klipolo.builtin/effect.blur' },
      ops: [{ type: 'updateEffect', target: { type: 'clip', id: 'a' }, id: 'x', patch: { enabled: false } }],
    });
    await expect(ops.removeEffect(stack, blur)).resolves.toBe(true);
    expect(lastTx()).toEqual({
      label: 'effect.remove.undo',
      params: { effect: 'klipolo.builtin/effect.blur' },
      ops: [{ type: 'removeEffect', target: { type: 'clip', id: 'a' }, id: 'x' }],
    });
    expect(clipA().effects).toEqual([]);
    await expect(ops.removeEffect(stack, blur)).resolves.toBe(false);
    expect(bridge.apply).toHaveBeenCalledTimes(2);
  });

  it('sets settings and takes others back to their defaults as one change', async () => {
    onClipA([fx('x', 'vignette', { amount: 0.8, softness: 0.2 })]);
    const stack = { target: { type: 'clip' as const, id: 'a' }, effects: clipA().effects };
    await ops.setEffectParams(stack, stack.effects[0], { amount: 0.3 }, ['softness']);
    expect(lastTx()).toEqual({
      label: 'effect.change.undo',
      params: { effect: 'klipolo.builtin/effect.vignette' },
      ops: [
        {
          type: 'updateEffect',
          target: { type: 'clip', id: 'a' },
          id: 'x',
          patch: { params: { amount: 0.3 }, reset: ['softness'] },
        },
      ],
    });
    expect(clipA().effects[0].params).toEqual({ amount: 0.3 });
    // Nothing to set: nothing is sent.
    await ops.setEffectParams(stack, stack.effects[0], {});
    expect(bridge.apply).toHaveBeenCalledTimes(1);
  });
});
