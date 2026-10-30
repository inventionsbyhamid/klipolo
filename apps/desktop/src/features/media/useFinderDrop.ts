import { getCurrentWebview } from '@tauri-apps/api/webview';
import { useEffect } from 'react';
import { importFiles } from './useImportMedia';

/**
 * Files dragged from Finder onto the window import as the Add button would (videos to the end of the main track, in
 * the order dropped). `onOver` hears while files are held over the window (true) and when they leave or land (false).
 * Outside Tauri (a plain browser, tests) nothing listens.
 */
export function useFinderDrop(onOver?: (over: boolean) => void) {
  useEffect(() => {
    let live = true;
    let off: (() => void) | undefined;
    let webview: ReturnType<typeof getCurrentWebview>;
    try {
      webview = getCurrentWebview();
    } catch {
      return;
    }
    webview
      .onDragDropEvent(({ payload }) => {
        if (payload.type === 'enter' || payload.type === 'over') onOver?.(true);
        else if (payload.type === 'leave') onOver?.(false);
        else {
          onOver?.(false);
          void importFiles(payload.paths);
        }
      })
      .then(
        (unlisten) => {
          if (live) off = unlisten;
          else unlisten();
        },
        // Finder drops would just stop working: say why in the log.
        (error: unknown) =>
          console.error('Finder drops are off: the drop listener could not be registered', error),
      );
    return () => {
      live = false;
      off?.();
      onOver?.(false);
    };
  }, [onOver]);
}
