import { act, fireEvent, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Asset } from '../../bindings/Asset';
import { fixtureSnapshot } from '../../test/fixtures';

/*
 * A drop over a lane that won't take what is let go says which lane does, from end to end: a library row dragged with
 * the pointer (`mediaDrag`), and files from Finder (`useFinderDrop`, which reads their kind as they come in).
 */

type Payload = { type: string; position?: { x: number; y: number }; paths?: string[] };
let deliver: (event: { payload: Payload }) => void = () => undefined;
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: (handler: typeof deliver) => {
      deliver = handler;
      return Promise.resolve(() => undefined);
    },
  }),
}));
vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), importMedia: vi.fn() },
  mediaUrl: (p: string) => p,
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { useToasts } = await import('../../state/toasts');
const { useUi } = await import('../../state/ui');
const { registerDropTarget, timelineLaneAt, timelineTargetAt } = await import('../timeline/dropTarget');
const { beginMediaDrag } = await import('../timeline/drag/mediaDrag');
const { useFinderDrop } = await import('./useFinderDrop');

/** A lane row from `top`, `height` px tall (jsdom has no layout). */
function row(attrs: Record<string, string>, top: number, height: number): HTMLElement {
  const el = document.createElement('div');
  for (const [name, value] of Object.entries(attrs)) el.setAttribute(name, value);
  el.getBoundingClientRect = () => DOMRect.fromRect({ x: 0, y: top, width: 1240, height });
  return el;
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useToasts.setState({ toasts: [] });
  useUi.setState({ pxPerSecond: 10, snapping: false });
  vi.mocked(bridge.importMedia).mockReset();
  // Overlays, Video, Sounds and Music under a 20 px ruler.
  const scroller = row({}, 0, 300);
  scroller.append(
    row({ 'data-lane': 'layers' }, 20, 24),
    row({ 'data-lane': 'main' }, 49, 64),
    row({ 'data-lane': 'sounds' }, 118, 22),
    row({ 'data-lane': 'music' }, 145, 34),
  );
  registerDropTarget(
    (x, y, kind) => timelineTargetAt(scroller, 20, x, y, kind),
    scroller,
    (x, y) => timelineLaneAt(scroller, 20, x, y),
  );
});
afterEach(() => registerDropTarget(null));

const texts = () => useToasts.getState().toasts.map((t) => t.text);

describe('a drop on a lane that won’t take it', () => {
  it('a library picture let go on the Video lane says it goes on Overlays, and adds nothing', () => {
    const logo = fixtureSnapshot().project.assets.find((a) => a.kind === 'image') as Asset;
    const onDrop = vi.fn();
    beginMediaDrag({ pointerId: 1, clientX: 100, clientY: 80 } as never, logo, onDrop);
    // Lifted past the slop, then let go over the Video lane.
    fireEvent.pointerMove(window, { pointerId: 1, clientX: 140, clientY: 80 });
    fireEvent.pointerUp(window, { pointerId: 1, clientX: 140, clientY: 80 });
    expect(onDrop).not.toHaveBeenCalled();
    expect(texts()).toEqual(['Pictures go on the Overlays lane.']);
  });

  it('a sound from Finder let go on the Overlays lane says it goes on Sounds or Music, and imports nothing', async () => {
    renderHook(() => useFinderDrop());
    await act(() => Promise.resolve());
    act(() => deliver({ payload: { type: 'enter', position: { x: 140, y: 30 }, paths: ['/pop.wav'] } }));
    act(() => deliver({ payload: { type: 'drop', position: { x: 140, y: 30 }, paths: ['/pop.wav'] } }));
    expect(texts()).toEqual(['Sounds go on the Sounds or Music lane.']);
    expect(bridge.importMedia).not.toHaveBeenCalled();
  });
});
