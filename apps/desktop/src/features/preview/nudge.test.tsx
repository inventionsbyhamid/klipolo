import { act, fireEvent, screen } from '@testing-library/react';
import { afterEach, beforeAll, beforeEach, expect, it, vi } from 'vitest';
import type { Op } from '../../bindings/Op';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

const apply = vi.fn<(label: string, ops: Op[]) => Promise<boolean>>(() => Promise.resolve(true));
vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn(), redo: vi.fn(), importMedia: vi.fn() },
  mediaUrl: (p: string) => p,
}));
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
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');
const { Stage } = await import('./Stage');

function EditorShortcutsProbe() {
  useEditorShortcuts();
  return null;
}

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
});

afterEach(() => vi.useRealTimers());

it('three arrow presses are one undo step; arrows elsewhere still step frames', async () => {
  vi.useFakeTimers();
  renderWithProviders(
    <>
      <Stage />
      <EditorShortcutsProbe />
    </>,
  );
  const g = screen.getByRole('button', { name: /^Text: Set up billing/ });
  g.focus();
  for (let i = 0; i < 3; i++) fireEvent.keyDown(g, { key: 'ArrowRight' });
  expect(apply).not.toHaveBeenCalled();
  act(() => vi.advanceTimersByTime(400));
  expect(apply).toHaveBeenCalledTimes(1);
  const [label, ops] = apply.mock.calls[0] as unknown as [
    string,
    { patch: { layout: { rect: { x: number } } } }[],
  ];
  expect(label).toBe('gizmo.nudge.undo');
  expect(ops[0].patch.layout.rect.x).toBeCloseTo(0.2 + 3 / 1920);
  expect(usePlayback.getState().playheadUs).toBe(2_000_000); // the gizmo had the keys
  fireEvent.keyDown(document.body, { key: 'ArrowRight' });
  expect(usePlayback.getState().playheadUs).toBe(2_033_333);
  await act(() => Promise.resolve());
});

it('a burst that ends where it began commits nothing', () => {
  vi.useFakeTimers();
  renderWithProviders(<Stage />);
  const g = screen.getByRole('button', { name: /^Text: Set up billing/ });
  g.focus();
  fireEvent.keyDown(g, { key: 'ArrowRight' });
  fireEvent.keyDown(g, { key: 'ArrowLeft' });
  act(() => vi.advanceTimersByTime(400));
  expect(apply).not.toHaveBeenCalled();
  expect(engine.previewClear).toHaveBeenCalled();
});
