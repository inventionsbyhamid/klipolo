import { act, fireEvent, screen } from '@testing-library/react';
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Layout } from '../../bindings/Layout';
import type { Op } from '../../bindings/Op';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * Moving an overlay on the canvas: in the primary format it moves the layer's rect; in another format only that
 * format's own place; a drag is one undo step however many moves it took; and near the canvas's centre line it snaps
 * there within 6 px, showing the guide.
 */

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
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { Stage } = await import('./Stage');

beforeAll(() => {
  // jsdom has no layout: give the stage an 800x450 box.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 450 });
});

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useSelection.setState({ selectedId: 'title' });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false, seq: 0 });
  useUi.setState({ previewFormat: null, safeArea: 'off' });
});

const gizmo = () => screen.getByRole('button', { name: /^Text: Set up billing/ });
const placed = () => (apply.mock.calls[0][1][0] as unknown as { patch: { layout: Layout } }).patch.layout;

/** Presses the title at (100, 100), moves it by (`dx`, `dy`) in `steps`, and lets go. */
function drag(dx: number, dy: number, steps = 1) {
  fireEvent.pointerDown(gizmo(), { button: 0, clientX: 100, clientY: 100, pointerId: 1 });
  for (let i = 1; i <= steps; i++)
    fireEvent.pointerMove(gizmo(), { clientX: 100 + (dx * i) / steps, clientY: 100 + (dy * i) / steps, pointerId: 1 });
  fireEvent.pointerUp(gizmo(), { pointerId: 1 });
}

describe('an overlay on the canvas', () => {
  it('drag_writes_rect_in_primary_format', async () => {
    renderWithProviders(<Stage />);
    drag(40, 45);
    const layout = placed();
    // 40 of 800 px and 45 of 450 px.
    expect(layout.rect.x).toBeCloseTo(0.25);
    expect(layout.rect.y).toBeCloseTo(0.18);
    expect(layout.overrides).toEqual({});
    await act(() => Promise.resolve());
  });

  it('drag_in_other_format_writes_override_only', async () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.settings.formats = ['landscape', 'portrait'];
    useProject.setState({ snapshot });
    useUi.setState({ previewFormat: 'portrait' });
    renderWithProviders(<Stage />);
    drag(0, 45);
    const layout = placed();
    expect(layout.rect).toEqual(titleItem.layout.rect);
    expect(layout.overrides.portrait).toBeDefined();
    expect(layout.overrides.landscape).toBeUndefined();
    await act(() => Promise.resolve());
  });

  it('drag_is_one_undo_step', async () => {
    renderWithProviders(<Stage />);
    drag(120, 60, 12);
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toBe('gizmo.move.undo');
    expect(apply.mock.calls[0][1]).toHaveLength(1);
    await act(() => Promise.resolve());
  });

  it('snap_to_centre_within_6px', async () => {
    // The title 8 px left of the centre line (0.6 wide from x 0.19).
    const snapshot = fixtureSnapshot();
    const [title] = snapshot.project.timeline.tracks[0].items;
    title.layout = { ...title.layout, rect: { ...title.layout.rect, x: 0.19 } };
    useProject.setState({ snapshot });
    renderWithProviders(<Stage />);
    const centred = () => screen.queryAllByTestId('snap-guide').some((g) => g.style.left === '50%');
    fireEvent.pointerDown(gizmo(), { button: 0, clientX: 100, clientY: 100, pointerId: 1 });
    // 4 px right: 4 px from the line, so it snaps onto it.
    fireEvent.pointerMove(gizmo(), { clientX: 104, clientY: 100, pointerId: 1 });
    expect(centred()).toBe(true);
    // 15 px right: 7 px past it, beyond the reach.
    fireEvent.pointerMove(gizmo(), { clientX: 115, clientY: 100, pointerId: 1 });
    expect(centred()).toBe(false);
    fireEvent.pointerMove(gizmo(), { clientX: 106, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(gizmo(), { pointerId: 1 });
    const { rect } = placed();
    expect(rect.x + rect.w / 2).toBeCloseTo(0.5, 9);
    await act(() => Promise.resolve());
  });
});
