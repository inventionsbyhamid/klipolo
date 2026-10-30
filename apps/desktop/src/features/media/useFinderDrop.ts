import { useEffect } from 'react';
import { onFileDrag } from '../../bridge/drop';
import { dropTargetAt, setDropIndicator } from '../timeline/drops';
import { importFiles } from './useImportMedia';

/**
 * Files dragged from Finder onto the window: over the timeline they show where they'll land and import there;
 * anywhere else they import as the Add button would (videos to the end of the main track, in the order dropped).
 * `onOver` hears while files are held over the window (true) and when they leave or land (false). Outside Tauri (a
 * plain browser) nothing listens.
 */
export function useFinderDrop(onOver?: (over: boolean) => void) {
  useEffect(() => {
    let live = true;
    let off: (() => void) | undefined;
    const listening = onFileDrag((drag) => {
      onOver?.(drag.type === 'over');
      const at = drag.type === 'leave' ? null : dropTargetAt(drag.x, drag.y);
      setDropIndicator(drag.type === 'over' ? at : null);
      if (drag.type === 'drop') void importFiles(drag.paths, at);
    });
    listening?.then(
      (unlisten) => {
        if (live) off = unlisten;
        else unlisten();
      },
      // Finder drops would just stop working: say why in the log.
      (error: unknown) => console.error('Finder drops are off: the drop listener could not be registered', error),
    );
    return () => {
      live = false;
      off?.();
      onOver?.(false);
      setDropIndicator(null);
    };
  }, [onOver]);
}
