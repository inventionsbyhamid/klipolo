import { act, screen } from '@testing-library/react';
import { Profiler } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { stubSize } from '../../test/layoutBox';
import { renderWithProviders } from '../../test/render';
import { captureResizeObservers } from '../../test/resizeObservers';

/*
 * The stage while its area resizes (a window resize, a panel that folds): the canvas's handles hide while the box they
 * were placed for differs from the one drawn, and the stage renders once at the settled size. Panel separators, a
 * timeline height bar and a view policy aren't tested here: Klipolo's panels don't resize by hand yet, and its preview
 * sends the view from the frame's own size.
 */

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    seek: vi.fn(() => Promise.resolve()),
    setView: vi.fn(() => Promise.resolve()),
    play: vi.fn(() => Promise.resolve()),
    pause: vi.fn(() => Promise.resolve(0)),
  },
}));
const { useProject } = await import('../../state/project');
const { useUi } = await import('../../state/ui');
const { useLayout } = await import('../../state/layout');
const { Stage } = await import('./Stage');

/** The stage's area, read live: a test changes its width, then fires the ResizeObservers. */
const area = { width: 800, height: 450 };
let observers: ReturnType<typeof captureResizeObservers>;
let unstub: () => void;
beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useUi.setState({ previewFormat: null });
  useLayout.setState({ resizing: false, groupPx: 0 });
  Object.assign(area, { width: 800, height: 450 });
  observers = captureResizeObservers();
  unstub = stubSize('[data-testid="stage-area"]', area);
});
afterEach(() => {
  unstub();
  observers.restore();
  vi.useRealTimers();
});
const resizeTo = (width: number) => {
  area.width = width;
  act(() => observers.fire());
};
const setResizing = (on: boolean) => act(() => useLayout.getState().setResizing(on));

describe('the preview while its area resizes', () => {
  it('the stage doesn’t re-render while a drag resizes it, and once after', () => {
    let renders = 0;
    renderWithProviders(
      <Profiler id="stage" onRender={() => (renders += 1)}>
        <Stage />
      </Profiler>,
    );
    const start = renders;
    setResizing(true);
    for (let i = 1; i <= 40; i++) resizeTo(800 - i * 5);
    expect(renders).toBe(start);
    setResizing(false);
    expect(renders).toBe(start + 1);
  });

  it('hides the handles while the size settles, and shows them after', () => {
    renderWithProviders(<Stage />);
    const handles = screen.getByTestId('stage-handles');
    setResizing(true);
    resizeTo(700);
    expect(handles).toHaveAttribute('data-settling');
    setResizing(false);
    expect(handles).not.toHaveAttribute('data-settling');
  });

  it('a 9:16 box that only moves keeps its handles while its area resizes', () => {
    const snapshot = fixtureSnapshot();
    snapshot.project.settings.formats = ['landscape', 'portrait'];
    useProject.setState({ snapshot });
    useUi.setState({ previewFormat: 'portrait' });
    renderWithProviders(<Stage />);
    const handles = screen.getByTestId('stage-handles');
    setResizing(true);
    // 450 px tall, the box is 253 px wide in every one of these: it only moves sideways.
    for (const width of [700, 500, 300]) resizeTo(width);
    expect(handles).not.toHaveAttribute('data-settling');
    // Narrower than the box: it shrinks, and the handles placed for the old size hide.
    resizeTo(200);
    expect(handles).toHaveAttribute('data-settling');
    setResizing(false);
    expect(handles).not.toHaveAttribute('data-settling');
  });

  it('a window resize settles 120 ms after its last change: the handles come back for the new size', () => {
    vi.useFakeTimers();
    renderWithProviders(<Stage />);
    const handles = screen.getByTestId('stage-handles');
    resizeTo(780);
    resizeTo(760);
    expect(handles).toHaveAttribute('data-settling');
    act(() => vi.advanceTimersByTime(119));
    expect(handles).toHaveAttribute('data-settling');
    act(() => vi.advanceTimersByTime(1));
    expect(handles).not.toHaveAttribute('data-settling');
  });
});
