import { act, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import type { ZoomTarget } from '../../bindings/ZoomTarget';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The cards of the layers chat and recordings make: a zoom (how far, and Smart or an area that stays inside the
 * frame), an adjustment layer (its effects change what is under it) and a generator (its settings from its schema).
 * Each change is one undo step, built from the layer as the project has it when it goes in.
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
const { engineApply, releaseSlider, withLayers } = await import('../../test/inspector');
const { Inspector } = await import('./Inspector');

const lastTx = () => vi.mocked(bridge.apply).mock.calls.at(-1)![0];
const testClock = () => vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));
const press = (el: Element, key: string) => fireEvent.keyDown(el, { key });

/** A zoom layer of `scale` to `target` on a zoom track of its own, selected. */
function withZoom(target: ZoomTarget, scale = 2) {
  const snapshot = fixtureSnapshot();
  const zoom: Item = {
    ...titleItem,
    id: 'zoom',
    attach: null,
    startUs: 2_000_000,
    content: { type: 'zoom', target, scale },
  };
  snapshot.project.timeline.tracks.push({ ...snapshot.project.timeline.tracks[0], id: 'z1', kind: 'zoom', items: [zoom] });
  useProject.setState({ snapshot, pending: 0 });
  useSelection.setState({ selectedId: 'zoom' });
}

const zoomOp = (content: object) => ({ type: 'updateItem', id: 'zoom', patch: { content: { type: 'zoom', ...content } } });

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  vi.mocked(bridge.apply).mockImplementation(engineApply());
});
afterEach(async () => {
  if (vi.isFakeTimers()) await vi.runAllTimersAsync();
  vi.useRealTimers();
});

describe("a zoom layer's card", () => {
  it('says how far it zooms, and Smart or an Area, which starts in the middle', async () => {
    withZoom({ type: 'auto' });
    renderWithProviders(<Inspector format="landscape" />);
    const card = screen.getByRole('complementary', { name: 'Zoom settings' });
    expect(within(card).getByRole('heading', { name: 'Zoom' })).toBeInTheDocument();
    expect(screen.getByRole('slider', { name: 'Zoom' })).toHaveAttribute('aria-valuetext', '2×');
    expect(screen.getByRole('radio', { name: 'Smart' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.queryByRole('slider', { name: 'Area across' })).toBeNull();
    // Its timing, and no effects: a zoom has none.
    expect(screen.getByRole('textbox', { name: 'Starts' })).toHaveValue('0:02.0');
    expect(screen.queryByRole('button', { name: /^Effects/ })).toBeNull();
    await userEvent.click(screen.getByRole('radio', { name: 'Area' }));
    expect(lastTx()).toEqual({
      label: 'zoom.undo',
      ops: [zoomOp({ scale: 2, target: { type: 'rect', rect: { x: 0.25, y: 0.25, w: 0.5, h: 0.5 } } })],
    });
  });

  it('an area moves across and down inside the frame, and keeps its centre as the zoom changes', async () => {
    withZoom({ type: 'rect', rect: { x: 0.25, y: 0.25, w: 0.5, h: 0.5 } });
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('slider', { name: 'Area across' })).toHaveAttribute('aria-valuetext', '50%');
    releaseSlider('Area across', 100);
    await act(async () => undefined);
    expect(lastTx()).toEqual({
      label: 'zoom.undo',
      ops: [zoomOp({ scale: 2, target: { type: 'rect', rect: { x: 0.5, y: 0.25, w: 0.5, h: 0.5 } } })],
    });
    // At 4x the area is a quarter of the frame, still round its centre (0.75, 0.5), kept inside the frame.
    testClock();
    const zoom = screen.getByRole('slider', { name: 'Zoom' });
    act(() => zoom.focus());
    for (let i = 0; i < 20; i++) press(zoom, 'ArrowRight');
    await advance(400);
    expect(lastTx()).toEqual({
      label: 'zoom.undo',
      ops: [zoomOp({ scale: 4, target: { type: 'rect', rect: { x: 0.625, y: 0.375, w: 0.25, h: 0.25 } } })],
    });
  });

  it('a smart zoom chat made from some kinds of action alone says so', () => {
    withZoom({ type: 'auto', using: ['clicks', 'typing'] });
    renderWithProviders(<Inspector format="landscape" />);
    const note = screen.getByRole('note');
    expect(note).toHaveTextContent('Only clicks and typing');
    expect(note).toHaveAccessibleDescription(
      'This zoom plans only from clicks and typing. Where it would hide pauses, that part stays in full view.',
    );
  });

  it('its more menu has no layer order: a zoom has its own track', async () => {
    withZoom({ type: 'auto' });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'More' }));
    expect(screen.getByRole('menuitem', { name: 'Duplicate' })).toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'Bring forward' })).toBeNull();
  });
});

describe("an adjustment layer's card", () => {
  it('says its effects change what is under it, and opens them', async () => {
    withLayers({ id: 'adjust', content: { type: 'adjustment' } });
    useSelection.setState({ selectedId: 'adjust' });
    renderWithProviders(<Inspector format="landscape" />);
    const card = screen.getByRole('complementary', { name: 'Adjustment layer settings' });
    expect(within(card).getByText("Effects here change what's under this layer")).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Effects' }));
    await userEvent.click(screen.getByRole('button', { name: 'Add effect' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Blur' }));
    expect(lastTx().ops[0]).toMatchObject({ type: 'addEffect', target: { type: 'item', id: 'adjust' } });
  });
});

describe("a generator layer's card", () => {
  const gradient = {
    id: 'grad',
    content: { type: 'generator' as const, source: 'klipolo.builtin/generator.gradient', params: { angle: 30 } },
  };

  it('has its settings from its schema, each one change named for it', async () => {
    withLayers(gradient);
    useSelection.setState({ selectedId: 'grad' });
    testClock();
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.getByRole('complementary', { name: 'Gradient settings' })).toBeInTheDocument();
    expect(screen.getByRole('radiogroup', { name: 'Start color' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Effects' })).toBeInTheDocument();
    const angle = screen.getByRole('slider', { name: 'Angle' });
    expect(angle).toHaveAttribute('aria-valuetext', '30°');
    act(() => angle.focus());
    press(angle, 'ArrowLeft');
    await advance(400);
    expect(lastTx()).toEqual({
      label: 'effect.change.undo',
      params: { effect: 'klipolo.builtin/generator.gradient' },
      ops: [
        {
          type: 'updateItem',
          id: 'grad',
          patch: {
            content: { type: 'generator', source: 'klipolo.builtin/generator.gradient', params: { angle: 29 } },
          },
        },
      ],
    });
  });

  it('a changed setting resets by taking it out, keeping the others', async () => {
    withLayers({ ...gradient, content: { ...gradient.content, params: { angle: 30, endColor: '#FFFFFFFF' } } });
    useSelection.setState({ selectedId: 'grad' });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Reset Angle' }));
    expect(lastTx().ops).toEqual([
      {
        type: 'updateItem',
        id: 'grad',
        patch: {
          content: { type: 'generator', source: 'klipolo.builtin/generator.gradient', params: { endColor: '#FFFFFFFF' } },
        },
      },
    ]);
  });

  it('one this version lacks is named as missing, its id as a tooltip', () => {
    withLayers({ id: 'odd', content: { type: 'generator', source: 'com.example/plasma', params: {} } });
    useSelection.setState({ selectedId: 'odd' });
    renderWithProviders(<Inspector format="landscape" />);
    const card = screen.getByRole('complementary', { name: 'Missing generator settings' });
    expect(within(card).getByText('Missing generator', { selector: 'p' })).toHaveAttribute('title', 'com.example/plasma');
    expect(screen.getByRole('button', { name: 'Effects' })).toBeInTheDocument();
  });
});

describe('zoom and other layers', () => {
  it('Smart after Area brings back what chat narrowed the smart zoom to', async () => {
    withZoom({ type: 'auto', using: ['pauses'] });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('radio', { name: 'Area' }));
    await userEvent.click(await screen.findByRole('radio', { name: 'Smart' }));
    expect(lastTx().ops).toEqual([zoomOp({ scale: 2, target: { type: 'auto', using: ['pauses'] } })]);
  });
});
