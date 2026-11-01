import { act } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { EngineInfo } from '../../bindings/EngineInfo';
import { fixtureSnapshot } from '../../test/fixtures';
import { stubSize } from '../../test/layoutBox';
import { renderWithProviders } from '../../test/render';

/*
 * A move to another screen changes the device pixel ratio, often with no resize at all. The preview then renders at
 * the new screen's pixels, and the native layer's place goes to Rust again at the new scale, so the video is never
 * soft on a Retina screen or oversized on a plain one.
 */

vi.mock('../../bridge/commands', () => ({ bridge: {}, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    setView: vi.fn(() => Promise.resolve()),
    setRect: vi.fn(() => Promise.resolve()),
    seek: vi.fn(() => Promise.resolve()),
    pause: vi.fn(() => Promise.resolve(0)),
  },
}));
const { engine } = await import('../../bridge/engine');
const { useEngine } = await import('../../state/engine');
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { Stage } = await import('./Stage');

const info: EngineInfo = {
  port: 4321,
  token: 't',
  maxPreviewPixels: 3840 * 2160,
  adapter: 'test',
  presenter: 'native',
  gpuClass: 'integrated',
};

/** The resolution queries listened to, by query: what a move between screens tells. */
const listeners = new Map<string, Set<() => void>>();
const original = { matchMedia: window.matchMedia, dpr: Object.getOwnPropertyDescriptor(window, 'devicePixelRatio') };
const setDpr = (dpr: number) =>
  Object.defineProperty(window, 'devicePixelRatio', { configurable: true, get: () => dpr });
/** The window moves to a screen of `dpr`: the query for the old ratio stops matching. */
function moveTo(dpr: number) {
  const was = window.devicePixelRatio;
  setDpr(dpr);
  for (const heard of [...(listeners.get(`(resolution: ${was}dppx)`) ?? [])]) heard();
}
const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));

let unstub: (() => void)[] = [];
beforeEach(() => {
  vi.clearAllMocks();
  listeners.clear();
  setDpr(1);
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      addEventListener: (_: string, heard: () => void) => {
        if (!listeners.has(query)) listeners.set(query, new Set());
        listeners.get(query)!.add(heard);
      },
      removeEventListener: (_: string, heard: () => void) => listeners.get(query)?.delete(heard),
    }) as unknown as MediaQueryList;
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  useUi.setState({ previewFormat: null });
  useEngine.setState({ info, presenter: 'native', status: 'live', message: null });
  unstub = [
    stubSize('[data-testid="stage-area"]', { width: 800, height: 450 }),
    stubSize('[data-testid="stage"]', { width: 640, height: 360, left: 80, top: 45 }),
  ];
});
afterEach(() => {
  for (const off of unstub) off();
  window.matchMedia = original.matchMedia;
  if (original.dpr) Object.defineProperty(window, 'devicePixelRatio', original.dpr);
  useEngine.setState({ info: null, presenter: 'socket', status: 'off' });
});

describe('a move to another screen', () => {
  it('renders at the new screen’s pixels with no resize, and back', () => {
    renderWithProviders(<Stage />);
    expect(engine.setView).toHaveBeenLastCalledWith({ format: 'landscape', width: 640, height: 360 });
    act(() => moveTo(2));
    expect(engine.setView).toHaveBeenLastCalledWith({ format: 'landscape', width: 1280, height: 720 });
    act(() => moveTo(1));
    expect(engine.setView).toHaveBeenLastCalledWith({ format: 'landscape', width: 640, height: 360 });
    expect(engine.setView).toHaveBeenCalledTimes(3);
  });

  it('sends the native layer’s place again at the new scale, never hiding it on the way', async () => {
    renderWithProviders(<Stage />);
    await frames();
    expect(engine.setRect).toHaveBeenLastCalledWith(
      expect.objectContaining({ x: 80, y: 45, width: 640, height: 360, scale: 1 }),
    );
    vi.mocked(engine.setRect).mockClear();
    act(() => moveTo(2));
    await frames();
    expect(vi.mocked(engine.setRect).mock.calls).toEqual([
      [expect.objectContaining({ x: 80, y: 45, width: 640, height: 360, scale: 2 })],
    ]);
  });
});
