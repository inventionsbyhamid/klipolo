import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Profiler } from 'react';
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CameraLayout } from '../../bindings/CameraLayout';
import type { Layout } from '../../bindings/Layout';
import type { Op } from '../../bindings/Op';
import { resolveRect } from '../../lib/layout';
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

describe('the camera bubble on the canvas', () => {
  type CameraOp = { type: string; id: string; patch: { camera: CameraLayout } };
  const camera = () => fixtureSnapshot().project.timeline.main[0].camera!;
  /** The one change a gesture sent: its label, the layer it names and the clip's camera it sets. */
  const sent = () => {
    expect(apply).toHaveBeenCalledTimes(1);
    const [label, ops, params] = apply.mock.calls[0] as unknown as [string, Op[], unknown];
    expect(ops).toHaveLength(1);
    expect(ops[0]).toMatchObject({ type: 'updateClip', id: 'a' });
    return { label, params, camera: (ops[0] as unknown as CameraOp).patch.camera };
  };

  it('selects with one press and drags in the same gesture: one move of the bubble in the main format', async () => {
    useSelection.setState({ selectedId: null });
    renderWithProviders(<Stage />);
    await userEvent.click(screen.getByRole('button', { name: 'Select Camera' }));
    expect(useSelection.getState().selectedId).toBe('camera:a');
    act(() => useSelection.setState({ selectedId: null }));
    const bubble = screen.getByRole('button', { name: 'Select Camera' });
    fireEvent.pointerDown(bubble, { button: 0, clientX: 700, clientY: 400, pointerId: 2 });
    expect(useSelection.getState().selectedId).toBe('camera:a');
    fireEvent.pointerMove(bubble, { clientX: 620, clientY: 400, pointerId: 2 });
    fireEvent.pointerUp(bubble, { pointerId: 2 });
    const { label, params, camera: moved } = sent();
    expect([label, params]).toEqual(['gizmo.move.undo', { layer: 'camera' }]);
    // 80 px of an 800 px canvas to the left; no other format gets a place of its own.
    expect(moved.bubble.rect.x).toBeCloseTo(0.66);
    expect(moved.bubble.overrides).toEqual({});
    // The rest of the camera as it was.
    expect({ ...moved, bubble: camera().bubble }).toEqual(camera());
    await act(() => Promise.resolve());
  });

  it('a move and a resize in 1:1 write the square place only', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.settings.formats = ['landscape', 'square'];
    useProject.setState({ snapshot });
    useUi.setState({ previewFormat: 'square' });
    useSelection.setState({ selectedId: 'camera:a' });
    renderWithProviders(<Stage />);
    const bubble = () => screen.getByRole('button', { name: /^Camera/ });
    // The square canvas is 450 px: 45 px is a tenth of it.
    const start = { x: parseFloat(bubble().style.left) / 450, w: parseFloat(bubble().style.width) / 450 };
    fireEvent.pointerDown(bubble(), { button: 0, clientX: 300, clientY: 300, pointerId: 1 });
    fireEvent.pointerMove(bubble(), { clientX: 255, clientY: 300, pointerId: 1, metaKey: true });
    fireEvent.pointerUp(bubble(), { pointerId: 1 });
    const move = sent();
    expect(move.label).toBe('gizmo.move.undo');
    expect(move.camera.bubble.rect).toEqual(camera().bubble.rect);
    expect(Object.keys(move.camera.bubble.overrides)).toEqual(['square']);
    expect(move.camera.bubble.overrides.square!.x).toBeCloseTo(start.x - 0.1);
    await act(() => Promise.resolve());

    apply.mockClear();
    fireEvent.pointerDown(bubble().querySelector('[data-handle="e"]')!, {
      button: 0,
      clientX: 300,
      clientY: 300,
      pointerId: 1,
    });
    fireEvent.pointerMove(bubble(), { clientX: 345, clientY: 300, pointerId: 1 });
    fireEvent.pointerUp(bubble(), { pointerId: 1 });
    const resize = sent();
    expect(resize.label).toBe('gizmo.resize.undo');
    expect(resize.camera.bubble.rect).toEqual(camera().bubble.rect);
    expect(Object.keys(resize.camera.bubble.overrides)).toEqual(['square']);
    expect(resize.camera.bubble.overrides.square!.w).toBeCloseTo(start.w + 0.1);
    await act(() => Promise.resolve());
  });

  it('as the split in 9:16 it selects but stays where the layout puts it', async () => {
    useUi.setState({ previewFormat: 'portrait' });
    useSelection.setState({ selectedId: 'camera:a' });
    renderWithProviders(<Stage />);
    const split = screen.getByRole('button', { name: 'Camera' });
    expect(split.querySelector('[data-handle]')).toBeNull();
    fireEvent.pointerDown(split, { button: 0, clientX: 200, clientY: 400, pointerId: 1 });
    fireEvent.pointerMove(split, { clientX: 200, clientY: 300, pointerId: 1 });
    fireEvent.pointerUp(split, { pointerId: 1 });
    expect(apply).not.toHaveBeenCalled();
  });

  it('is not on the canvas where it is hidden, nor in a take that is the camera alone', () => {
    const hidden = fixtureSnapshot();
    hidden.project.timeline.main[0].camera!.mode = 'hidden';
    useProject.setState({ snapshot: hidden });
    const { unmount } = renderWithProviders(<Stage />);
    expect(screen.queryByRole('button', { name: /Camera/ })).toBeNull();
    unmount();

    const alone = fixtureSnapshot();
    const rec = alone.project.recordings[0];
    rec.sources = rec.sources.filter((s) => s.id !== 'screen');
    rec.primary = 'camera';
    rec.kind = 'camera';
    useProject.setState({ snapshot: alone });
    renderWithProviders(<Stage />);
    expect(screen.queryByRole('button', { name: /Camera/ })).toBeNull();
  });

  it('as a bubble in 9:16 a drag writes the 9:16 place only, leaving the layouts as they are', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.settings.formats = ['landscape', 'portrait'];
    snapshot.project.timeline.main[0].camera!.modeOverrides = { portrait: 'bubble' };
    useProject.setState({ snapshot });
    useUi.setState({ previewFormat: 'portrait' });
    useSelection.setState({ selectedId: 'camera:a' });
    renderWithProviders(<Stage />);
    const bubble = screen.getByRole('button', { name: /^Camera/ });
    // Placed from 16:9: a 384 px square on 1080 × 1920.
    const start = resolveRect(camera().bubble, 'landscape', 'portrait');
    fireEvent.pointerDown(bubble, { button: 0, clientX: 200, clientY: 300, pointerId: 1 });
    fireEvent.pointerMove(bubble, { clientX: 150, clientY: 300, pointerId: 1, metaKey: true });
    fireEvent.pointerUp(bubble, { pointerId: 1 });
    const { label, camera: moved } = sent();
    expect(label).toBe('gizmo.move.undo');
    expect(moved.bubble.rect).toEqual(camera().bubble.rect);
    expect(Object.keys(moved.bubble.overrides)).toEqual(['portrait']);
    const placed = moved.bubble.overrides.portrait!;
    expect(placed.x).toBeLessThan(start.x - 0.1);
    expect([placed.y, placed.w, placed.h].map((v) => +v.toFixed(6))).toEqual(
      [start.y, start.w, start.h].map((v) => +v.toFixed(6)),
    );
    expect([moved.mode, moved.modeOverrides]).toEqual(['bubble', { portrait: 'bubble' }]);
    await act(() => Promise.resolve());
  });

  it('a run of arrow keys is one Nudge camera step, in the format shown only', async () => {
    vi.useFakeTimers();
    const snapshot = fixtureSnapshot();
    snapshot.project.settings.formats = ['landscape', 'square'];
    useProject.setState({ snapshot });
    useUi.setState({ previewFormat: 'square' });
    useSelection.setState({ selectedId: 'camera:a' });
    renderWithProviders(<Stage />);
    const bubble = screen.getByRole('button', { name: /^Camera/ });
    act(() => bubble.focus());
    for (let i = 0; i < 3; i++) fireEvent.keyDown(bubble, { key: 'ArrowRight' });
    expect(apply).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(400));
    const { label, params, camera: nudged } = sent();
    expect([label, params]).toEqual(['gizmo.nudge.undo', { layer: 'camera' }]);
    expect(nudged.bubble.rect).toEqual(camera().bubble.rect);
    expect(Object.keys(nudged.bubble.overrides)).toEqual(['square']);
    // Three 1 px steps of 1080.
    const start = resolveRect(camera().bubble, 'landscape', 'square');
    expect(nudged.bubble.overrides.square!.x).toBeCloseTo(start.x + 3 / 1080, 9);
    expect(nudged.bubble.overrides.square!.y).toBeCloseTo(start.y, 9);
    expect([nudged.mode, nudged.modeOverrides]).toEqual([camera().mode, camera().modeOverrides]);
    vi.useRealTimers();
    await act(() => Promise.resolve());
  });
});
