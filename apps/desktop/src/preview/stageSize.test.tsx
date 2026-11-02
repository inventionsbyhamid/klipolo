import { act, fireEvent, screen, within } from '@testing-library/react';
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
 * keep their size and place: the card shows in the left column, over the library, and nothing in the preview makes
 * room for it. Selecting, deselecting and walking with the arrows never make the video jump.
 */

vi.mock('../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../bridge/library', () => ({
  library: {
    pack: vi.fn(() => Promise.resolve([])),
    marks: vi.fn(() => Promise.resolve({})),
    picture: vi.fn(() => Promise.reject(new Error('no pictures in tests'))),
    speechRanges: vi.fn(() => Promise.resolve([])),
  },
}));
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
const { holdDock, releaseDock, useDockHold } = await import('../state/dockHold');
const { useEngine } = await import('../state/engine');
const { useFullScreen } = await import('../state/fullScreen');
const { useProject } = await import('../state/project');
const { usePlayback } = await import('../state/playback');
const { useSelection } = await import('../state/selection');
const { useTimelineFocus } = await import('../state/timelineFocus');
const { useUi } = await import('../state/ui');
const { Preview } = await import('./Preview');
const { Library } = await import('../layout/Library');

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
  useFullScreen.setState({ on: false });
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

/**
 * What sizes and places the video: the preview's own markup down to the video box (every class, style and attribute),
 * and the area's, the video box's and the handles' attributes.
 */
function stageBox() {
  const el = (id: string) => screen.getByTestId(id);
  const attrs = (node: HTMLElement) => [node.className, node.getAttribute('style'), node.dataset.card ?? null];
  const preview = screen.getByRole('main', { name: 'Preview' });
  return {
    body: (preview.querySelector('.stage-body')!.cloneNode(false) as HTMLElement).outerHTML,
    children: [...preview.querySelector('.stage-main')!.children].map((c) => c.className),
    area: attrs(el('stage-area')),
    frame: attrs(el('stage')),
    handles: attrs(el('stage-handles')),
  };
}

/** The card, wherever it is. */
const card = () => screen.queryByRole('complementary', { name: /settings$/ });
/** The card in the left column, over the library. */
const columnCard = () => {
  const library = document.querySelector<HTMLElement>('.library')!;
  return within(library).queryByRole('complementary', { name: /settings$/ });
};

function Editor() {
  return (
    <>
      <Library />
      <Preview />
    </>
  );
}

describe('selecting never resizes the video', () => {
  it('a 16:9 video keeps its size and place as a clip is selected, deselected and selected again', () => {
    renderWithProviders(<Editor />);
    act(() => observers.fire());
    const before = stageBox();
    const views = vi.mocked(engine.setView).mock.calls.length;
    expect(views).toBeGreaterThan(0);
    act(() => useSelection.getState().select('a'));
    expect(columnCard()).toHaveAccessibleName('Recording 1 settings');
    // Nothing in the preview: no card, no slot.
    expect(within(screen.getByRole('main', { name: 'Preview' })).queryByRole('complementary')).toBeNull();
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

  it('a 16:9 video keeps its whole area with a card shown: nothing is kept clear for it', () => {
    useSelection.setState({ selectedId: 'a' });
    renderWithProviders(<Editor />);
    const stage = screen.getByTestId('stage-area');
    expect(stage).not.toHaveAttribute('data-card');
    expect(stage.style.paddingRight).toBe('');
    expect(columnCard()).toBeInTheDocument();
  });

  it('a 9:16 video stays centred as a title is selected; no note or slot beside it', () => {
    useUi.setState({ previewFormat: 'portrait' });
    Object.assign(frame, { width: 253, height: 450 });
    renderWithProviders(<Editor />);
    const stage = screen.getByTestId('stage-area');
    expect(stage.style.paddingRight).toBe('');
    expect(screen.queryByText('Select something on the video to change it.')).toBeNull();
    const before = stageBox();
    act(() => useSelection.getState().select('title'));
    expect(columnCard()).toHaveAccessibleName('Title settings');
    expect(stageBox()).toEqual(before);
    act(() => useSelection.getState().select(null));
    expect(card()).toBeNull();
    expect(stageBox()).toEqual(before);
  });

  it('a press holds the card back and the stage keeps its box, before and after the release', () => {
    useUi.setState({ previewFormat: 'portrait' });
    Object.assign(frame, { width: 253, height: 450 });
    renderWithProviders(<Editor />);
    const before = stageBox();
    act(() => {
      holdDock();
      useSelection.getState().select('title');
    });
    expect(card()).toBeNull();
    expect(stageBox()).toEqual(before);
    act(() => releaseDock());
    expect(columnCard()).toHaveAccessibleName('Title settings');
    expect(stageBox()).toEqual(before);
  });

  it('full screen shows no card: the 9:16 video is centred', () => {
    useUi.setState({ previewFormat: 'portrait' });
    useFullScreen.setState({ on: true });
    renderWithProviders(<Editor />);
    expect(screen.getByTestId('stage-area').style.paddingRight).toBe('');
    act(() => useSelection.getState().select('title'));
    expect(card()).toBeNull();
  });

  it('walking the lanes with the arrows never moves the video', async () => {
    useSelection.setState({ selectedId: 'a' });
    const { Timeline } = await import('../features/timeline/Timeline');
    renderWithProviders(
      <>
        <Editor />
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
    expect(css).not.toMatch(/has-inspector|stage-hint|data-card="(slot|over)"/);
    // The card's own rules never reach into the preview.
    expect(css).not.toMatch(/(\.screen|\.frame|\.stage)[^{]*\.inspector|\.inspector[^{]*(\.screen|\.frame|\.stage\b)/);
  });
});

/* A drag on the canvas never moves the stage, with the card in the left column throughout. */
describe('a drag on the video', () => {
  it('a drag of the title down the video keeps the stage box and the card in the column', async () => {
    const apply = vi.fn(() => Promise.resolve(true));
    useProject.setState({ apply });
    usePlayback.setState({ playheadUs: 2_000_000 });
    useSelection.setState({ selectedId: 'title' });
    renderWithProviders(<Editor />);
    const before = stageBox();
    const title = () => screen.getByRole('button', { name: /^Text: Set up billing/ });
    fireEvent.pointerDown(title(), { button: 0, clientX: 400, clientY: 60, pointerId: 1 });
    for (const y of [120, 240, 400]) {
      fireEvent.pointerMove(title(), { clientX: 400, clientY: y, pointerId: 1 });
      await act(() => new Promise((r) => setTimeout(r, 20)));
      expect(columnCard()).toHaveAccessibleName('Title settings');
      expect(stageBox()).toEqual(before);
    }
    fireEvent.pointerUp(title(), { pointerId: 1 });
    expect(apply).toHaveBeenCalledTimes(1);
    expect(stageBox()).toEqual(before);
    await act(() => Promise.resolve());
  });
});
