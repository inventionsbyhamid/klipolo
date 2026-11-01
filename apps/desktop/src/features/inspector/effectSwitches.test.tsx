import { act, fireEvent, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ContribDef } from '../../bindings/ContribDef';
import { BUILTINS } from '../../lib/contrib';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * An effect's switch and a switch setting: each press flips what the project has when it goes in, so two quick
 * presses turn it one way and back, the second built on the first.
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
const { engineApply, fx, onClipA, StackOf } = await import('../../test/inspector');

const testClock = () => vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

/** An effect with a switch setting, which no built-in has: put in the catalogue for a test and taken out after it. */
const INVERT: ContribDef = {
  id: 'com.example/invert',
  kind: 'effect',
  name: 'Invert',
  params: [{ key: 'flip', label: 'Flip', type: 'bool', default: false }],
};

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  vi.mocked(bridge.apply).mockImplementation(engineApply());
});
afterEach(async () => {
  if (vi.isFakeTimers()) await vi.runAllTimersAsync();
  vi.useRealTimers();
});

describe('two quick presses on a switch', () => {
  beforeEach(() => void BUILTINS.push(INVERT));
  afterEach(() => void BUILTINS.splice(BUILTINS.indexOf(INVERT), 1));

  it('an effect’s switch turns it off and back on, the second press built on the first', async () => {
    testClock();
    vi.mocked(bridge.apply).mockImplementation(engineApply(50));
    onClipA([fx('x', 'blur')]);
    renderWithProviders(<StackOf target={{ type: 'clip', id: 'a' }} />);
    const blur = screen.getByRole('switch', { name: 'Blur' });
    fireEvent.click(blur);
    fireEvent.click(blur);
    await advance(300);
    expect(vi.mocked(bridge.apply).mock.calls.map(([tx]) => [tx.label, tx.ops[0]])).toEqual([
      ['effect.off.undo', { type: 'updateEffect', target: { type: 'clip', id: 'a' }, id: 'x', patch: { enabled: false } }],
      ['effect.on.undo', { type: 'updateEffect', target: { type: 'clip', id: 'a' }, id: 'x', patch: { enabled: true } }],
    ]);
    expect(useProject.getState().snapshot!.project.timeline.main[0].effects[0].enabled).toBe(true);
  });

  it('a switch setting turns on and back off', async () => {
    testClock();
    vi.mocked(bridge.apply).mockImplementation(engineApply(50));
    onClipA([{ id: 'i', effect: 'com.example/invert', enabled: true, params: {} }]);
    renderWithProviders(<StackOf target={{ type: 'clip', id: 'a' }} />);
    fireEvent.click(screen.getByRole('button', { name: 'Show Invert settings' }));
    const flip = screen.getByRole('switch', { name: 'Flip' });
    fireEvent.click(flip);
    fireEvent.click(flip);
    await advance(300);
    expect(vi.mocked(bridge.apply).mock.calls.map(([tx]) => tx.ops[0])).toEqual([
      { type: 'updateEffect', target: { type: 'clip', id: 'a' }, id: 'i', patch: { params: { flip: true } } },
      { type: 'updateEffect', target: { type: 'clip', id: 'a' }, id: 'i', patch: { params: { flip: false } } },
    ]);
  });
});
