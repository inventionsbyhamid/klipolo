import { act, fireEvent, screen } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { EngineInfo } from '../bindings/EngineInfo';
import { fixtureSnapshot } from '../test/fixtures';
import { stubSize } from '../test/layoutBox';
import { renderWithProviders } from '../test/render';
import { captureResizeObservers } from '../test/resizeObservers';

/*
 * Selecting never resizes the video. A plain click selects and shows its card, but the stage and the engine's drawable
 * keep their size and place: the card shows in a slot of its own beside the video, or over the video's right side when
 * a slot would cost the video too much of its size. Selecting, deselecting and walking with the arrows never make the
 * video jump.
 */

vi.mock('../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../bridge/engine', () => ({
  engine: {
    setView: vi.fn(() => Promise.resolve()),
    setRect: vi.fn(() => Promise.resolve()),
    preview: vi.fn(() => Promise.resolve()),
    previewClear: vi.fn(() => Promise.resolve()),
    seek: vi.fn(() => Promise.resolve()),
    play: vi.fn(() => Promise.resolve()),
    pause: vi.fn(() => Promise.resolve(0)),
  },
}));
vi.mock('../bridge/framing', () => ({
  clipFraming: vi.fn(() => new Promise(() => undefined)),
}));
const { engine } = await import('../bridge/engine');
const { releaseDock, useDockHold } = await import('../state/dockHold');
const { useEngine } = await import('../state/engine');
const { useProject } = await import('../state/project');
const { usePlayback } = await import('../state/playback');
const { useSelection } = await import('../state/selection');
const { useTimelineFocus } = await import('../state/timelineFocus');
const { useUi } = await import('../state/ui');
const { Preview } = await import('./Preview');

const info: EngineInfo = {
  port: 4321,
  token: 't',
  maxPreviewPixels: 3840 * 2160,
  adapter: 'test',
  presenter: 'native',
  gpuClass: 'integrated',
};

/** The stage's area, and the video's box in it. */
const area = { width: 800, height: 450 };
const frame = { width: 800, height: 450 };
let observers: ReturnType<typeof captureResizeObservers>;
let unstub: (() => void)[] = [];

beforeEach(() => {
  releaseDock();
  useDockHold.setState({ walk: false });
  const snapshot = fixtureSnapshot();
  snapshot.project.settings.formats = ['landscape', 'portrait'];
  useProject.setState({ snapshot, pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 25_000_000, playing: false });
  useTimelineFocus.setState({ activeId: null, pending: false });
  useUi.setState({ previewFormat: null, timelineExpanded: false, pxPerSecond: 40, snapping: true });
  useEngine.setState({ info, presenter: 'native', status: 'live', message: null });
  Object.assign(area, { width: 800, height: 450 });
  Object.assign(frame, { width: 800, height: 450 });
  observers = captureResizeObservers();
  unstub = [stubSize('[data-testid="stage-area"]', area), stubSize('[data-testid="stage"]', frame)];
  vi.mocked(engine.setView).mockClear();
});
afterEach(() => {
  for (const off of unstub) off();
  observers.restore();
  useEngine.setState({ info: null, presenter: 'socket', status: 'off' });
});

/** What sizes and places the video: the area's and the video box's attributes, and the canvas the layers are placed on. */
function stageBox() {
  const el = (id: string) => screen.getByTestId(id);
  const attrs = (node: HTMLElement) => [node.className, node.getAttribute('style'), node.dataset.card ?? null];
  return {
    area: attrs(el('stage-area')),
    frame: attrs(el('stage')),
    handles: attrs(el('stage-handles')),
  };
}

const card = () => screen.queryByRole('complementary');

describe('selecting never resizes the video', () => {
  it('a 16:9 video keeps its size and place as a clip is selected, deselected and selected again', () => {
    renderWithProviders(<Preview />);
    act(() => observers.fire());
    const before = stageBox();
    const views = vi.mocked(engine.setView).mock.calls.length;
    expect(views).toBeGreaterThan(0);
    act(() => useSelection.getState().select('a'));
    expect(card()).toHaveAccessibleName('Recording 1 settings');
    act(() => observers.fire());
    expect(stageBox()).toEqual(before);
    act(() => useSelection.getState().select(null));
    expect(card()).toBeNull();
    act(() => useSelection.getState().select('b'));
    act(() => observers.fire());
    expect(stageBox()).toEqual(before);
    // The engine's drawable: never sent a new size.
    expect(vi.mocked(engine.setView).mock.calls).toHaveLength(views);
  });

  it('a 16:9 video that a slot would shrink by more than a fifth: the card floats over it', () => {
    useSelection.setState({ selectedId: 'a' });
    renderWithProviders(<Preview />);
    expect(screen.getByTestId('stage-area')).toHaveAttribute('data-card', 'over');
    expect(screen.getByTestId('stage-area').style.paddingRight).toBe('');
    expect(card()).toBeInTheDocument();
  });

  it('a 9:16 video has the slot beside it: the card shows there', () => {
    useUi.setState({ previewFormat: 'portrait' });
    Object.assign(frame, { width: 253, height: 450 });
    renderWithProviders(<Preview />);
    const stage = screen.getByTestId('stage-area');
    expect(stage).toHaveAttribute('data-card', 'slot');
    // Moved left only as far as the 264 px card and its 16 px gap need.
    expect(stage.style.paddingRight).toBe('13px');
    const before = stageBox();
    act(() => useSelection.getState().select('title'));
    expect(card()).toHaveAccessibleName('Title settings');
    expect(stageBox()).toEqual(before);
  });

  it('walking the lanes with the arrows never moves the video', async () => {
    useSelection.setState({ selectedId: 'a' });
    const { Timeline } = await import('../features/timeline/Timeline');
    renderWithProviders(
      <>
        <Preview />
        <Timeline />
      </>,
    );
    act(() => observers.fire());
    const before = stageBox();
    const views = vi.mocked(engine.setView).mock.calls.length;
    act(() => screen.getByRole('button', { name: /^Recording 1/ }).focus());
    for (const key of ['ArrowRight', 'ArrowRight', 'ArrowLeft', 'ArrowLeft'] as const) {
      fireEvent.keyDown(document.activeElement!, { key });
      await act(() => new Promise((r) => setTimeout(r, 40)));
      expect(stageBox()).toEqual(before);
    }
    await act(() => new Promise((r) => setTimeout(r, 450)));
    act(() => observers.fire());
    expect(stageBox()).toEqual(before);
    expect(vi.mocked(engine.setView).mock.calls).toHaveLength(views);
  });

  it('no style sizes or places the video by whether a card is shown', () => {
    const css = readFileSync(resolve(__dirname, '../styles/app.css'), 'utf8');
    expect(css).not.toMatch(/has-inspector[^{]*\.(screen|frame)/);
  });
});
