import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The native preview's hole: its place goes to Rust, and the window's own colour around it is left to the theme (Rust
 * paints it with the theme's canvas), so the hole never paints the window dark in the light theme.
 */

vi.mock('../../bridge/commands', () => ({ bridge: {}, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    seek: vi.fn(() => Promise.resolve()),
    setView: vi.fn(() => Promise.resolve()),
    setRect: vi.fn(() => Promise.resolve()),
  },
}));
const { engine } = await import('../../bridge/engine');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useEngine } = await import('../../state/engine');
const { Stage } = await import('./Stage');

const box = HTMLElement.prototype.getBoundingClientRect;

beforeAll(() => {
  // jsdom has no layout: every box is 800 x 450 at (10, 20), and a frame is drawn at once.
  HTMLElement.prototype.getBoundingClientRect = () =>
    ({ x: 10, y: 20, left: 10, top: 20, right: 810, bottom: 470, width: 800, height: 450 }) as DOMRect;
  vi.stubGlobal('requestAnimationFrame', (draw: FrameRequestCallback) => {
    draw(0);
    return 1;
  });
});

afterAll(() => {
  HTMLElement.prototype.getBoundingClientRect = box;
  vi.unstubAllGlobals();
});

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false, seq: 0 });
  useEngine.setState({
    status: 'live',
    message: null,
    presenter: 'native',
    info: { port: 1, token: 't', maxPreviewPixels: 0, adapter: '', presenter: 'native', gpuClass: 'software' },
  });
});

describe('the native preview’s hole', () => {
  it('sends its place and leaves the window’s colour to the theme', () => {
    renderWithProviders(<Stage />);
    expect(engine.setRect).toHaveBeenCalledWith({ x: 10, y: 20, width: 800, height: 450, scale: 1, backdrop: null });
  });
});
