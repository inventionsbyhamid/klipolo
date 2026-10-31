import { describe, expect, it, vi } from 'vitest';

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
const { onFileDrag } = await import('./drop');

describe('onFileDrag', () => {
  it('says where files are held and dropped in page px, and their paths as they come in and in order', () => {
    Object.defineProperty(window, 'devicePixelRatio', { configurable: true, value: 2 });
    const seen: unknown[] = [];
    expect(onFileDrag((drag) => seen.push(drag))).not.toBeNull();
    deliver({ payload: { type: 'enter', position: { x: 200, y: 100 }, paths: ['/a.mp4'] } });
    deliver({ payload: { type: 'over', position: { x: 300, y: 120 } } });
    deliver({ payload: { type: 'leave' } });
    deliver({ payload: { type: 'drop', position: { x: 40, y: 60 }, paths: ['/a.mp4', '/b.mov'] } });
    expect(seen).toEqual([
      { type: 'over', x: 100, y: 50, paths: ['/a.mp4'] },
      { type: 'over', x: 150, y: 60 },
      { type: 'leave' },
      { type: 'drop', paths: ['/a.mp4', '/b.mov'], x: 20, y: 30 },
    ]);
  });
});
