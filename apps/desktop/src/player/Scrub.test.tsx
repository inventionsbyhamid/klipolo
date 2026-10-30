import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';

const bridge = vi.hoisted(() => ({
  engineSeek: vi.fn(() => Promise.resolve()),
  engineAttach: vi.fn(() =>
    Promise.resolve({ port: 1, token: 't', maxPreviewPixels: 1, adapter: 'a', presenter: 'socket', gpuClass: 'apple' }),
  ),
  projectSummary: vi.fn(() => Promise.resolve({ name: 'clip.mp4', durationUs: 10_000_000 })),
  enginePlay: vi.fn(() => Promise.resolve(0)),
  enginePause: vi.fn(() => Promise.resolve(0)),
}));
vi.mock('../bridge/engine', () => bridge);

import { Scrub } from './Scrub';
import { startSession } from './session';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

beforeAll(async () => {
  window.requestAnimationFrame = (run: FrameRequestCallback) => {
    run(0);
    return 0;
  };
  await startSession();
});

afterEach(() => {
  document.body.replaceChildren();
});

describe('Scrub', () => {
  it('seeks to where the timeline is clicked, with a newer sequence each time', async () => {
    const host = document.createElement('div');
    document.body.append(host);
    const root = createRoot(host);
    await act(async () => root.render(<Scrub />));
    const bar = host.querySelector<HTMLElement>('[role="slider"]')!;
    bar.getBoundingClientRect = () => ({ left: 100, width: 200, top: 0, height: 20 }) as DOMRect;
    bar.setPointerCapture = () => {};
    const seq = bridge.engineSeek.mock.calls.length;

    await act(async () => {
      bar.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, clientX: 150, button: 0 }));
      bar.dispatchEvent(new MouseEvent('pointerup', { bubbles: true, clientX: 150 }));
    });
    await act(async () => {
      bar.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, clientX: 400, button: 0 }));
    });

    const calls = bridge.engineSeek.mock.calls.slice(seq) as unknown as [number, number][];
    expect(calls.map(([at]) => at)).toEqual([2_500_000, 10_000_000]);
    expect(calls[1][1]).toBeGreaterThan(calls[0][1]);
    expect(bar.getAttribute('aria-valuetext')).toBe('0:10.0');
    root.unmount();
  });
});
