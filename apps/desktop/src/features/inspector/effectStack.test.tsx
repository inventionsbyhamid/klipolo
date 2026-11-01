import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * A clip's or layer's effect stack: its effects top to bottom in the order they run, each with a switch, its settings
 * behind a disclosure, and a menu to move it or remove it; then Add effect, up to the model's eight. Every change is
 * one undo step, built from the stack as the project has it when it goes in.
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
const { engineApply, fx, onClipA, releaseSlider, StackOf, withLayers } = await import('../../test/inspector');
const { EffectStack } = await import('./EffectStack');

const lastTx = () => vi.mocked(bridge.apply).mock.calls.at(-1)![0];
const testClock = () => vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));
const press = (el: Element, key: string) => fireEvent.keyDown(el, { key });

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  vi.mocked(bridge.apply).mockImplementation(engineApply());
});
afterEach(async () => {
  // A commit still on its way lands, so the next test's edits don't queue behind it.
  if (vi.isFakeTimers()) await vi.runAllTimersAsync();
  vi.useRealTimers();
});

describe('an effect stack', () => {
  it('shows an effect’s settings from its schema, and commits one change per gesture', async () => {
    testClock();
    onClipA([fx('fx', 'blur')]);
    renderWithProviders(<StackOf target={{ type: 'clip', id: 'a' }} />);
    expect(screen.queryByRole('slider', { name: 'Radius' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Show Blur settings' }));
    expect(screen.getByRole('button', { name: 'Hide Blur settings' })).toHaveAttribute('aria-expanded', 'true');
    const radius = screen.getByRole('slider', { name: 'Radius' });
    expect(radius).toHaveAttribute('aria-valuetext', '12 px');
    act(() => radius.focus());
    press(radius, 'ArrowRight');
    press(radius, 'ArrowRight');
    expect(bridge.apply).not.toHaveBeenCalled();
    await advance(400);
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(lastTx()).toEqual({
      label: 'effect.change.undo',
      params: { effect: 'klipolo.builtin/effect.blur' },
      ops: [{ type: 'updateEffect', target: { type: 'clip', id: 'a' }, id: 'fx', patch: { params: { radius: 14 } } }],
    });
  });

  it('adds, switches off, moves and removes effects', async () => {
    const { unmount } = renderWithProviders(<EffectStack target={{ type: 'item', id: 'title' }} effects={[]} />);
    expect(screen.getByText('No effects yet')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Add effect' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Vignette' }));
    expect(lastTx()).toEqual({
      label: 'effect.add.undo',
      params: { effect: 'klipolo.builtin/effect.vignette' },
      ops: [
        {
          type: 'addEffect',
          target: { type: 'item', id: 'title' },
          effect: { id: '', effect: 'klipolo.builtin/effect.vignette', enabled: true, params: {} },
          index: null,
        },
      ],
    });
    unmount();
    withLayers({ id: 'logo-layer', content: { type: 'image', assetId: 'logo' }, effects: [fx('a', 'blur'), fx('b', 'vignette')] });
    renderWithProviders(<StackOf target={{ type: 'item', id: 'logo-layer' }} />);
    await userEvent.click(screen.getByRole('switch', { name: 'Blur' }));
    expect(lastTx()).toEqual({
      label: 'effect.off.undo',
      params: { effect: 'klipolo.builtin/effect.blur' },
      ops: [{ type: 'updateEffect', target: { type: 'item', id: 'logo-layer' }, id: 'a', patch: { enabled: false } }],
    });
    await waitFor(() => expect(screen.getByRole('switch', { name: 'Blur' })).not.toBeChecked());
    await userEvent.click(screen.getByRole('button', { name: 'Vignette options' }));
    expect(screen.getByRole('menuitem', { name: 'Move down' })).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(screen.getByRole('menuitem', { name: 'Move up' }));
    expect(lastTx()).toEqual({
      label: 'effect.up.undo',
      params: { effect: 'klipolo.builtin/effect.vignette' },
      ops: [{ type: 'moveEffect', target: { type: 'item', id: 'logo-layer' }, id: 'b', index: 0 }],
    });
    await waitFor(() =>
      expect(screen.getAllByRole('switch').map((s) => s.getAttribute('aria-label'))).toEqual(['Vignette', 'Blur']),
    );
    await userEvent.click(screen.getByRole('button', { name: 'Blur options' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Remove' }));
    expect(lastTx()).toEqual({
      label: 'effect.remove.undo',
      params: { effect: 'klipolo.builtin/effect.blur' },
      ops: [{ type: 'removeEffect', target: { type: 'item', id: 'logo-layer' }, id: 'a' }],
    });
  });

  it('a removed effect hands focus to the row now in its place, never to the page', async () => {
    onClipA([fx('x', 'blur'), fx('y', 'glow')]);
    renderWithProviders(<StackOf target={{ type: 'clip', id: 'a' }} />);
    await userEvent.click(screen.getByRole('button', { name: 'Blur options' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Remove' }));
    await waitFor(() => expect(screen.queryByRole('switch', { name: 'Blur' })).toBeNull());
    await waitFor(() => expect(screen.getByRole('switch', { name: 'Glow' })).toHaveFocus());
  });

  it('shows an effect this build lacks by its id, and can only remove it', () => {
    const missing = { id: 'fx', effect: 'com.example.neon/glow', enabled: true, params: {} };
    renderWithProviders(<EffectStack target={{ type: 'clip', id: 'a' }} effects={[missing]} />);
    expect(screen.getByText('Missing effect')).toHaveAttribute('title', 'com.example.neon/glow');
    expect(screen.queryByRole('button', { name: /settings/ })).toBeNull();
    expect(screen.queryByRole('switch')).toBeNull();
    expect(screen.getByRole('button', { name: 'Remove missing effect' })).toBeInTheDocument();
  });

  it('a stack of eight takes no more, says why, and numbers effects of a kind', () => {
    const eight = Array.from({ length: 8 }, (_, i) => fx(`fx${i}`, 'vignette'));
    renderWithProviders(<EffectStack target={{ type: 'clip', id: 'a' }} effects={eight} />);
    const add = screen.getByRole('button', { name: 'Add effect' });
    expect(add).toHaveAttribute('aria-disabled', 'true');
    act(() => add.focus());
    expect(add).toHaveAccessibleDescription('A clip or layer can have up to eight effects');
    fireEvent.click(add);
    expect(screen.queryByRole('menu')).toBeNull();
    expect(screen.getByRole('switch', { name: 'Vignette (8)' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Vignette (2) options' })).toBeInTheDocument();
  });

  it('a changed setting resets to its default as one change', async () => {
    onClipA([fx('v', 'vignette', { amount: 0.8 })]);
    renderWithProviders(<StackOf target={{ type: 'clip', id: 'a' }} />);
    fireEvent.click(screen.getByRole('button', { name: 'Show Vignette settings' }));
    await userEvent.click(screen.getByRole('button', { name: 'Reset Amount' }));
    expect(lastTx()).toEqual({
      label: 'effect.change.undo',
      params: { effect: 'klipolo.builtin/effect.vignette' },
      ops: [{ type: 'updateEffect', target: { type: 'clip', id: 'a' }, id: 'v', patch: { reset: ['amount'] } }],
    });
  });
});

describe('a quick second change builds on the first', () => {
  it('a point: across, then at once down, while the first change is still on its way', async () => {
    testClock();
    vi.mocked(bridge.apply).mockImplementation(engineApply(50));
    onClipA([fx('m', 'mask')]);
    renderWithProviders(<StackOf target={{ type: 'clip', id: 'a' }} />);
    fireEvent.click(screen.getByRole('button', { name: 'Show Shape mask settings' }));
    const across = screen.getByRole('slider', { name: 'Center across' });
    const down = screen.getByRole('slider', { name: 'Center down' });
    act(() => across.focus());
    press(across, 'ArrowRight');
    // On to Center down (as Tab does): leaving commits across, which lands 50 ms later; a press there straight away.
    act(() => down.focus());
    await act(async () => undefined);
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    press(down, 'ArrowRight');
    await advance(500);
    expect(bridge.apply).toHaveBeenCalledTimes(2);
    expect(vi.mocked(bridge.apply).mock.calls[1][0].ops).toEqual([
      {
        type: 'updateEffect',
        target: { type: 'clip', id: 'a' },
        id: 'm',
        patch: { params: { center: { x: 0.51, y: 0.51 } } },
      },
    ]);
  });

  it('a slider release, then at once a switch and the other axis of the same effect: every change is kept', async () => {
    testClock();
    vi.mocked(bridge.apply).mockImplementation(engineApply(50));
    onClipA([fx('m', 'mask')]);
    renderWithProviders(<StackOf target={{ type: 'clip', id: 'a' }} />);
    fireEvent.click(screen.getByRole('button', { name: 'Show Shape mask settings' }));
    releaseSlider('Center across', 40);
    fireEvent.click(screen.getByRole('switch', { name: 'Shape mask' }));
    releaseSlider('Center down', 70);
    await advance(200);
    expect(vi.mocked(bridge.apply).mock.calls.map(([tx]) => tx.label)).toEqual([
      'effect.change.undo',
      'effect.off.undo',
      'effect.change.undo',
    ]);
    expect(useProject.getState().snapshot!.project.timeline.main[0].effects[0]).toMatchObject({
      enabled: false,
      params: { center: { x: 0.4, y: 0.7 } },
    });
  });
});
