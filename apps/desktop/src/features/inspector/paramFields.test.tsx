import { act, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ContribDef } from '../../bindings/ContribDef';
import type { Op } from '../../bindings/Op';
import type { ParamValue } from '../../bindings/ParamValue';
import { BUILTINS } from '../../lib/contrib';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * A contribution's settings, generated from its schema alone: one labelled control per setting by its type, hidden
 * while its `visibleWhen` doesn't hold, with Reset once it differs from its default. Each pick is one change; a slider
 * is one change per gesture.
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
const { engineApply, releaseSlider } = await import('../../test/inspector');
const { ParamFields } = await import('./ParamFields');

const lastTx = () => vi.mocked(bridge.apply).mock.calls.at(-1)![0];
const def = (id: string) => BUILTINS.find((d) => d.id === `klipolo.builtin/${id}`)!;
const testClock = () => vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));
const press = (el: Element, key: string) => fireEvent.keyDown(el, { key });

/** Each change commits a stand-in op that names the setting and its value. */
const renderFields = (contrib: ContribDef, values: Record<string, ParamValue> = {}) =>
  renderWithProviders(
    <ParamFields
      def={contrib}
      values={values}
      opsFor={(key, value) => [{ type: 'rename', name: `${key}=${JSON.stringify(value)}` }]}
      commitLabel="effect.change.undo"
    />,
  );

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  vi.mocked(bridge.apply).mockImplementation(engineApply());
});
afterEach(async () => {
  if (vi.isFakeTimers()) await vi.runAllTimersAsync();
  vi.useRealTimers();
});

describe('settings generated from a schema', () => {
  it('a number is a slider showing its unit, one change per run of keys', async () => {
    testClock();
    renderFields(def('effect.blur'));
    const radius = screen.getByRole('slider', { name: 'Radius' });
    expect(radius).toHaveAttribute('aria-valuetext', '12 px');
    act(() => radius.focus());
    press(radius, 'ArrowRight');
    press(radius, 'ArrowRight');
    // Whole pixels a press, as shown.
    expect(radius).toHaveAttribute('aria-valuetext', '14 px');
    expect(bridge.apply).not.toHaveBeenCalled();
    await advance(400);
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(lastTx()).toEqual({ label: 'effect.change.undo', ops: [{ type: 'rename', name: 'radius=14' }] });
  });

  it('shows angles in degrees, shares in percent and short pixel ranges in tenths', () => {
    renderFields(def('effect.drop-shadow'));
    expect(screen.getByRole('slider', { name: 'Angle' })).toHaveAttribute('aria-valuetext', '45°');
    expect(screen.getByRole('slider', { name: 'Softness' })).toHaveAttribute('aria-valuetext', '60%');
    renderFields(def('effect.mask'));
    expect(screen.getByRole('slider', { name: 'Feather' })).toHaveAttribute('aria-valuetext', '1 px');
  });

  it('a point is two sliders, across and down, one change per gesture', async () => {
    testClock();
    const spot: ContribDef = {
      id: 'com.example/spot',
      kind: 'effect',
      name: 'Spot',
      params: [{ key: 'center', label: 'Spot', type: 'point', default: { x: 0.5, y: 0.5 } }],
    };
    renderFields(spot);
    expect(screen.getByRole('group', { name: 'Spot' })).toBeInTheDocument();
    const across = screen.getByRole('slider', { name: 'Spot across' });
    expect(across).toHaveAttribute('aria-valuetext', '50%');
    expect(screen.getByRole('slider', { name: 'Spot down' })).toBeInTheDocument();
    act(() => across.focus());
    for (let i = 0; i < 3; i++) press(across, 'ArrowRight');
    await advance(400);
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(lastTx()).toEqual({
      label: 'effect.change.undo',
      ops: [{ type: 'rename', name: 'center={"x":0.53,"y":0.5}' }],
    });
  });

  it('a setting whose condition doesn’t hold is hidden, and its stored value stays (the Shape mask)', async () => {
    const mask = def('effect.mask');
    const circle = renderFields(mask);
    expect(screen.queryByRole('slider', { name: 'Corner radius' })).toBeNull();
    expect(screen.getByRole('slider', { name: 'Center across' })).toBeInTheDocument();
    circle.unmount();
    // A frame is the whole layer: corners, no centre. Its stored centre isn't touched by a shape change.
    renderFields(mask, { shape: 'frame', center: { x: 0.2, y: 0.5 } });
    expect(screen.getByRole('slider', { name: 'Corner radius' })).toBeInTheDocument();
    expect(screen.queryByRole('group', { name: 'Center' })).toBeNull();
    await userEvent.click(screen.getByRole('radio', { name: 'Circle' }));
    expect(lastTx()).toEqual({ label: 'effect.change.undo', ops: [{ type: 'rename', name: 'shape="circle"' }] });
  });

  it('names colours and centres as the rest of the app writes them', () => {
    renderFields(def('generator.aurora'));
    for (const name of ['Sky color', 'First color', 'Second color', 'Third color'])
      expect(screen.getByRole('radiogroup', { name })).toBeInTheDocument();
    expect(screen.queryByText(/colour/i)).toBeNull();
  });

  it('a colour is swatches; a swatch keeps the alpha the setting holds, and Opacity changes only that', async () => {
    renderFields(def('effect.drop-shadow'));
    const color = screen.getByRole('radiogroup', { name: 'Color' });
    // #000000A6: black, at 65%.
    expect(within(color).getByRole('radio', { name: 'Black' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('slider', { name: 'Color opacity' })).toHaveAttribute('aria-valuetext', '65%');
    await userEvent.click(within(color).getByRole('radio', { name: 'White' }));
    expect(lastTx()).toEqual({ label: 'effect.change.undo', ops: [{ type: 'rename', name: 'color="#FFFFFFA6"' }] });
    releaseSlider('Color opacity', 40);
    await act(async () => undefined);
    expect(lastTx()).toEqual({ label: 'effect.change.undo', ops: [{ type: 'rename', name: 'color="#00000066"' }] });
  });

  it('a colour without an alpha has no Opacity, and takes the swatch as it is', async () => {
    renderFields(def('effect.glow'));
    expect(screen.queryByRole('slider', { name: /opacity/ })).toBeNull();
    await userEvent.click(within(screen.getByRole('radiogroup', { name: 'Color' })).getByRole('radio', { name: 'Yellow' }));
    expect(lastTx()).toEqual({ label: 'effect.change.undo', ops: [{ type: 'rename', name: 'color="#FFD02E"' }] });
  });

  it('a choice of more than four is a select; a switch is a toggle; words commit on Enter', async () => {
    const many: ContribDef = {
      id: 'com.example/many',
      kind: 'effect',
      name: 'Many',
      params: [
        { key: 'corner', label: 'Corner', type: 'choice', options: ['a', 'b', 'c', 'd', 'top-left'], default: 'a' },
        { key: 'invert', label: 'Invert', type: 'bool', default: false },
        { key: 'caption', label: 'Caption', type: 'text', default: 'Hi', maxLen: 20 },
      ],
    };
    renderFields(many);
    await userEvent.click(screen.getByRole('button', { name: 'Corner: A' }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Top left' }));
    expect(lastTx()).toEqual({ label: 'effect.change.undo', ops: [{ type: 'rename', name: 'corner="top-left"' }] });
    await userEvent.click(screen.getByRole('switch', { name: 'Invert' }));
    expect(lastTx()).toEqual({ label: 'effect.change.undo', ops: [{ type: 'rename', name: 'invert=true' }] });
    const words = screen.getByRole('textbox', { name: 'Caption' });
    await userEvent.clear(words);
    await userEvent.type(words, 'Bye{Escape}');
    expect(words).toHaveValue('Hi');
    await userEvent.clear(words);
    await userEvent.type(words, 'Bye{Enter}');
    expect(lastTx()).toEqual({ label: 'effect.change.undo', ops: [{ type: 'rename', name: 'caption="Bye"' }] });
  });

  it('a kind with no control here says so', () => {
    const table: ContribDef = {
      id: 'com.example/chart',
      kind: 'effect',
      name: 'Chart',
      params: [{ key: 'rows', label: 'Rows', type: 'table', columns: ['a'], maxRows: 4, default: [] }],
    };
    renderFields(table);
    expect(screen.getByText("Can't be changed in this version")).toBeInTheDocument();
  });

  it('a changed setting can be reset, and focus stays on the setting', async () => {
    const reset = vi.fn((key: string): Op[] => [{ type: 'rename', name: `reset ${key}` }]);
    renderWithProviders(
      <ParamFields
        def={def('effect.vignette')}
        values={{ amount: 0.8 }}
        opsFor={() => []}
        resetOps={reset}
        commitLabel={{ id: 'effect.change.undo', params: { effect: 'klipolo.builtin/effect.vignette' } }}
      />,
    );
    expect(screen.queryByRole('button', { name: 'Reset Softness' })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Reset Amount' }));
    expect(lastTx()).toEqual({
      label: 'effect.change.undo',
      params: { effect: 'klipolo.builtin/effect.vignette' },
      ops: [{ type: 'rename', name: 'reset amount' }],
    });
    expect(screen.getByRole('slider', { name: 'Amount' })).toHaveFocus();
  });
});
