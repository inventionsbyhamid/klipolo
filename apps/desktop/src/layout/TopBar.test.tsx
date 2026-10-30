import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { describe, expect, it, vi } from 'vitest';

const bridge = vi.hoisted(() => ({
  engineSeek: vi.fn(() => Promise.resolve()),
  engineAttach: vi.fn(() =>
    Promise.resolve({ port: 1, token: 't', maxPreviewPixels: 1, adapter: 'a', presenter: 'socket', gpuClass: 'apple' }),
  ),
  projectSummary: vi.fn(() => Promise.resolve(null)),
  enginePlay: vi.fn(),
  enginePause: vi.fn(),
  engineSetVisible: vi.fn(() => Promise.resolve()),
}));
vi.mock('../bridge/engine', () => bridge);

import { startSession } from '../player/session';
import { TopBar } from './TopBar';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe('TopBar', () => {
  it('says why Export is off, with and without a video', async () => {
    const host = document.createElement('div');
    document.body.append(host);
    const root = createRoot(host);
    await act(async () => root.render(<TopBar />));
    const exportButton = () => [...host.querySelectorAll('button')].find((b) => b.textContent === 'Export')!;
    expect(exportButton().disabled).toBe(true);
    expect(exportButton().title).toBe('Add a video first');

    bridge.projectSummary.mockResolvedValueOnce({ name: 'clip.mp4', durationUs: 1_000_000 } as never);
    await act(async () => startSession());
    expect(exportButton().title).toBe('Export comes in the next build.');
    root.unmount();
  });
});
