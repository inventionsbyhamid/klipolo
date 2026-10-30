import type { UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';

/**
 * Files dragged onto the window from Finder: held over it (with where, in page px), gone again, or dropped (their
 * paths, in Finder's order).
 */
export type FileDrag =
  | { type: 'over'; x: number; y: number }
  | { type: 'leave' }
  | { type: 'drop'; paths: string[]; x: number; y: number };

/**
 * Hears files dragged onto the window. Tauri reports the drag in physical pixels from the webview's top left; this says
 * page px. Null outside Tauri (a plain browser), where nothing listens.
 */
export function onFileDrag(handler: (drag: FileDrag) => void): Promise<UnlistenFn> | null {
  let webview: ReturnType<typeof getCurrentWebview>;
  try {
    webview = getCurrentWebview();
  } catch {
    return null;
  }
  return webview.onDragDropEvent(({ payload }) => {
    const dpr = window.devicePixelRatio || 1;
    if (payload.type === 'leave') return handler({ type: 'leave' });
    const x = payload.position.x / dpr;
    const y = payload.position.y / dpr;
    if (payload.type === 'drop') handler({ type: 'drop', paths: payload.paths, x, y });
    else handler({ type: 'over', x, y });
  });
}
