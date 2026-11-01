import { act, fireEvent, screen } from '@testing-library/react';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Layout } from '../../bindings/Layout';
import type { Op } from '../../bindings/Op';
import { fixtureSnapshot } from '../../test/fixtures';
import { stubSize } from '../../test/layoutBox';
import { renderWithProviders } from '../../test/render';

/*
 * The rotate knob (a knob on a stem above the top edge): a selected text, image or video
 * layer turns about its centre as the knob is dragged round it, snapping to upright and the quarter turns within 3
 * degrees, in 15 degree steps with Shift. One change on release, undone as "rotate". The camera and title templates
 * don't turn.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: unknown) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
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
  // jsdom has no layout: give the stage an 800×450 box.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 450 });
});

/** The title's box on screen, centred at 400, 70. */
let unstub: () => void;
beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useSelection.setState({ selectedId: 'title' });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false, seq: 0 });
  useUi.setState({ previewFormat: null });
  unstub = stubSize('[data-layer="title"]', { width: 480, height: 40, left: 160, top: 50 });
});
afterEach(() => unstub());

const title = () => screen.getByRole('button', { name: /^Text: Set up billing/ });
const knob = (layer: HTMLElement) => layer.querySelector<HTMLElement>('[data-handle="rotate"]');
/** Press the knob straight above the centre, drag to `x`, `y`, and let go. */
function turn(x: number, y: number, shiftKey = false) {
  fireEvent.pointerDown(knob(title())!, { button: 0, clientX: 400, clientY: 20, pointerId: 1 });
  fireEvent.pointerMove(title(), { clientX: x, clientY: y, pointerId: 1, shiftKey });
  fireEvent.pointerUp(title(), { pointerId: 1 });
}
const turned = () => {
  expect(apply).toHaveBeenCalledTimes(1);
  const [label, ops, params] = apply.mock.calls[0];
  expect([label, params]).toEqual(['gizmo.rotate.undo', { layer: 'text' }]);
  expect(ops).toHaveLength(1);
  expect(ops[0]).toMatchObject({ type: 'updateItem', id: 'title' });
  return (ops[0] as unknown as { patch: { layout: Layout } }).patch.layout;
};

describe('the rotate knob', () => {
  it('a selected title has a knob; dragging it a quarter turn round the centre turns the title by 90 degrees', async () => {
    renderWithProviders(<Stage />);
    expect(knob(title())).toBeInTheDocument();
    turn(450, 70);
    const layout = turned();
    expect(layout.rotation).toBe(90);
    // A turn alone leaves the box where it was.
    expect(layout.rect).toEqual(fixtureSnapshot().project.timeline.tracks[0].items[0].layout.rect);
    await act(() => Promise.resolve());
  });

  it('turns freely to a tenth of a degree, and in 15 degree steps with Shift', async () => {
    renderWithProviders(<Stage />);
    // From straight up to 21.8 degrees above the right: a 68.2 degree turn.
    turn(450, 50);
    expect(turned().rotation).toBe(68.2);
    await act(() => Promise.resolve());
    apply.mockClear();
    turn(450, 50, true);
    expect(turned().rotation).toBe(75);
    await act(() => Promise.resolve());
  });

  it('the turned title is drawn turned as soon as it is let go', async () => {
    renderWithProviders(<Stage />);
    turn(450, 70);
    expect(title().style.transform).toContain('rotate(90deg)');
    await act(() => Promise.resolve());
  });

  it('no knob while playing, on a title not selected, or on the camera', () => {
    useSelection.setState({ selectedId: null });
    renderWithProviders(<Stage />);
    expect(knob(screen.getByRole('button', { name: 'Select Set up billing' }))).toBeNull();
    act(() => useSelection.setState({ selectedId: 'title' }));
    act(() => usePlayback.setState({ playing: true }));
    expect(knob(title())).toBeNull();
    act(() => usePlayback.setState({ playing: false }));
    act(() => useSelection.setState({ selectedId: 'camera:a' }));
    const camera = screen.getByRole('button', { name: /^Camera/ });
    expect(camera.querySelector('[data-handle="se"]')).toBeInTheDocument();
    expect(knob(camera)).toBeNull();
  });
});
