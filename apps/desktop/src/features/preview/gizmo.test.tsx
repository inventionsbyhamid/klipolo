import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Profiler } from 'react';
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Layout } from '../../bindings/Layout';
import type { Op } from '../../bindings/Op';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

const apply = vi.fn<(label: string, ops: Op[]) => Promise<boolean>>(() => Promise.resolve(true));
vi.mock('../../bridge/commands', () => ({ bridge: {}, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    seek: vi.fn(() => Promise.resolve()),
    setView: vi.fn(() => Promise.resolve()),
    pause: vi.fn(() => Promise.resolve(0)),
    preview: vi.fn(() => Promise.resolve()),
    previewClear: vi.fn(() => Promise.resolve()),
  },
}));
const { engine } = await import('../../bridge/engine');
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { useToasts } = await import('../../state/toasts');
const { useUi } = await import('../../state/ui');
const { Stage } = await import('./Stage');

beforeAll(() => {
  // jsdom has no layout: give the stage an 800×450 box.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 450 });
});

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useSelection.setState({ selectedId: 'title' });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false, seq: 0 });
  useToasts.setState({ toasts: [] });
  useUi.setState({ previewFormat: null });
});

type Patch = { patch: { layout: Layout; content?: unknown } };

const gizmo = () => screen.getByRole('button', { name: /^Text: Set up billing/ });
type Move = { patch: { layout: { rect: { x: number } } } };

describe('canvas gizmo', () => {
  it('drags a layer with a live preview of the exact op, without re-rendering, and commits one move', async () => {
    const renders = vi.fn();
    renderWithProviders(
      <Profiler id="preview" onRender={renders}>
        <Stage />
      </Profiler>,
    );
    fireEvent.pointerDown(gizmo(), { button: 0, clientX: 100, clientY: 100, pointerId: 1 });
    fireEvent.pointerMove(gizmo(), { clientX: 140, clientY: 100, pointerId: 1 });
    const commits = renders.mock.calls.length;
    fireEvent.pointerMove(gizmo(), { clientX: 180, clientY: 100, pointerId: 1 });
    expect(renders).toHaveBeenCalledTimes(commits); // the box follows by transform
    expect(gizmo().style.transform).toBe('translate(80px, 0px)');
    await waitFor(() => expect(engine.preview).toHaveBeenCalled());
    fireEvent.pointerUp(gizmo(), { pointerId: 1 });
    expect(apply).toHaveBeenCalledTimes(1);
    const [label, ops] = apply.mock.calls[0];
    expect(label).toBe('gizmo.move.undo');
    expect((ops[0] as unknown as Move).patch.layout.rect.x).toBeCloseTo(0.3); // 80 px of an 800 px canvas
    expect(vi.mocked(engine.preview).mock.calls.at(-1)).toEqual(['project', 3, ops]);
    // The committed box takes over from the transform in the same frame.
    expect(parseFloat(gizmo().style.left)).toBeCloseTo(240);
    expect(gizmo().style.transform).toBe('');
    expect(engine.previewClear).not.toHaveBeenCalled();
    await act(() => Promise.resolve());
  });

  it('Escape mid-drag cancels without an edit', () => {
    renderWithProviders(<Stage />);
    fireEvent.pointerDown(gizmo(), { button: 0, clientX: 100, clientY: 100, pointerId: 1 });
    fireEvent.pointerMove(gizmo(), { clientX: 160, clientY: 120, pointerId: 1 });
    fireEvent.keyDown(gizmo(), { key: 'Escape' });
    fireEvent.pointerUp(gizmo(), { pointerId: 1 });
    expect(engine.previewClear).toHaveBeenCalled();
    expect(apply).not.toHaveBeenCalled();
    expect(gizmo().style.transform).toBe('');
    expect(useSelection.getState().selectedId).toBe('title');
  });

  it('a bare video click picks the clip; outside the canvas clears', async () => {
    renderWithProviders(<Stage />);
    await userEvent.click(screen.getByTestId('stage'));
    expect(useSelection.getState().selectedId).toBe('a');
    await userEvent.click(screen.getByTestId('stage').parentElement!);
    expect(useSelection.getState().selectedId).toBeNull();
  });

  it('a layer that leaves the canvas mid-drag drops its dry run', () => {
    renderWithProviders(<Stage />);
    fireEvent.pointerDown(gizmo(), { button: 0, clientX: 100, clientY: 100, pointerId: 1 });
    fireEvent.pointerMove(gizmo(), { clientX: 160, clientY: 100, pointerId: 1 });
    act(() => usePlayback.setState({ playheadUs: 5_000_000 })); // past the title (1 s to 4 s)
    expect(screen.queryByRole('button', { name: /^Text: Set up billing/ })).toBeNull();
    expect(engine.previewClear).toHaveBeenCalledTimes(1);
    expect(apply).not.toHaveBeenCalled();
  });

  it('a text corner scales the box and its font in one change', async () => {
    renderWithProviders(<Stage />);
    // The title is 480×54 px on the 800×450 canvas; this drag is 10 % bigger along its diagonal.
    fireEvent.pointerDown(gizmo().querySelector('[data-handle="se"]')!, {
      button: 0,
      clientX: 100,
      clientY: 100,
      pointerId: 1,
    });
    fireEvent.pointerMove(gizmo(), { clientX: 148, clientY: 105.4, pointerId: 1 });
    expect(parseFloat(gizmo().style.width)).toBeCloseTo(528); // drawn without a render
    fireEvent.pointerUp(gizmo(), { pointerId: 1 });
    expect(apply).toHaveBeenCalledTimes(1);
    const [label, ops] = apply.mock.calls[0];
    expect(label).toBe('gizmo.resize.undo');
    expect(ops).toHaveLength(1);
    expect(ops[0]).toMatchObject({
      type: 'updateItem',
      id: 'title',
      patch: { content: { style: { size: 79 } } },
    });
    const rect = (ops[0] as unknown as { patch: { layout: { rect: Record<string, number> } } }).patch.layout
      .rect;
    expect([rect.x, rect.y, rect.w, rect.h].map((v) => +v.toFixed(6))).toEqual([0.2, 0.08, 0.66, 0.132]);
    await act(() => Promise.resolve());
  });

  it('a layer on a locked track says so once per press and stays put', () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.tracks[0].locked = true;
    useProject.setState({ snapshot });
    useSelection.setState({ selectedId: null });
    renderWithProviders(<Stage />);
    const layer = screen.getByRole('button', { name: 'Select Set up billing, locked' });
    fireEvent.pointerDown(layer, { button: 0, clientX: 100, clientY: 100, pointerId: 1 });
    fireEvent.pointerMove(layer, { clientX: 140, clientY: 100, pointerId: 1 });
    fireEvent.pointerMove(layer, { clientX: 180, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(layer, { pointerId: 1 });
    expect(useToasts.getState().toasts.map((t) => t.text)).toEqual(['This track is locked.']);
    expect(useSelection.getState().selectedId).toBe('title');
    expect(apply).not.toHaveBeenCalled();
    expect(engine.preview).not.toHaveBeenCalled();
  });

  it('a turned layer keeps a tenth of its turned bounds on the canvas', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.tracks[0].items[0].layout.rotation = 90;
    useProject.setState({ snapshot });
    renderWithProviders(<Stage />);
    fireEvent.pointerDown(gizmo(), { button: 0, clientX: 400, clientY: 100, pointerId: 1 });
    fireEvent.pointerMove(gizmo(), { clientX: -2000, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(gizmo(), { pointerId: 1 });
    const { rect } = (apply.mock.calls[0][1][0] as unknown as Patch).patch.layout;
    // Upright, the 0.6 × 0.12 title (1152 × 130 px) is 0.0675 of the canvas wide: a tenth of that stays on.
    const half = (0.12 * 1080) / 1920 / 2;
    expect(rect.x + rect.w / 2 + half).toBeGreaterThanOrEqual(0.1 * 2 * half - 1e-9);
    await act(() => Promise.resolve());
  });

  it("in another format a text corner changes only that format's box (the font follows it)", async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.settings.formats = ['landscape', 'portrait'];
    useProject.setState({ snapshot });
    useUi.setState({ previewFormat: 'portrait' });
    renderWithProviders(<Stage />);
    fireEvent.pointerDown(gizmo().querySelector('[data-handle="se"]')!, {
      button: 0,
      clientX: 100,
      clientY: 100,
      pointerId: 1,
    });
    fireEvent.pointerMove(gizmo(), { clientX: 120, clientY: 104, pointerId: 1 });
    fireEvent.pointerUp(gizmo(), { pointerId: 1 });
    const [label, ops] = apply.mock.calls[0];
    expect(label).toBe('gizmo.resize.undo');
    const { patch } = ops[0] as unknown as Patch;
    expect(patch.content).toBeUndefined();
    expect(patch.layout.rect).toEqual(titleItem.layout.rect);
    expect(patch.layout.overrides.portrait!.h).toBeGreaterThan(0.06);
    await act(() => Promise.resolve());
  });

  it('a move in 9:16 snaps to the safe area shown last, with the overlay off', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.settings.formats = ['landscape', 'portrait'];
    useProject.setState({ snapshot });
    useUi.setState({ previewFormat: 'portrait', safeArea: 'off', lastSafeArea: 'reels' });
    renderWithProviders(<Stage />);
    // Reels leaves 210 of 1920 px clear at the top: the title's top is let go 1 px under it on the 450 px canvas.
    const clearTop = 210 / 1920;
    const by = clearTop * 450 + 1 - parseFloat(gizmo().style.top);
    fireEvent.pointerDown(gizmo(), { button: 0, clientX: 100, clientY: 100, pointerId: 1 });
    fireEvent.pointerMove(gizmo(), { clientX: 100, clientY: 100 + by, pointerId: 1 });
    expect(screen.getByTestId('safe-area')).toHaveAttribute('data-platform', 'reels');
    fireEvent.pointerUp(gizmo(), { pointerId: 1 });
    const { patch } = apply.mock.calls[0][1][0] as unknown as Patch;
    expect(patch.layout.overrides.portrait!.y).toBeCloseTo(clearTop, 6);
    await act(() => Promise.resolve());
  });
});
