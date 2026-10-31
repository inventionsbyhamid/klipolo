import { useEffect } from 'react';
import { onFileDrag } from '../../bridge/drop';
import type { AssetKind } from '../../bindings/AssetKind';
import { toast } from '../../state/toasts';
import { dropRefusal, dropTargetAt, kindOfPaths, setDropIndicator } from '../timeline/drops';
import { importFiles } from './useImportMedia';

/**
 * Files dragged from Finder onto the window: over the timeline they show where they'll land, on the lanes that take
 * what they are (by their extensions, read as they come in), and import there; let go over a lane that won't take them,
 * they say which lane does and nothing is added; anywhere else they import as the Add button would (videos to the end
 * of the main track, in the order dropped). `onOver` hears while files are held over the window (true) and when they
 * leave or land (false). Outside Tauri (a plain browser) nothing listens.
 */
export function useFinderDrop(onOver?: (over: boolean) => void) {
  useEffect(() => {
    let live = true;
    let off: (() => void) | undefined;
    /** What the files held over the window are: one kind, or null (mixed, or not known yet). */
    let kind: AssetKind | null = null;
    const listening = onFileDrag((drag) => {
      onOver?.(drag.type === 'over');
      if (drag.type !== 'leave' && drag.paths) kind = kindOfPaths(drag.paths);
      const at = drag.type === 'leave' ? null : dropTargetAt(drag.x, drag.y, kind);
      setDropIndicator(drag.type === 'over' ? at : null);
      if (drag.type === 'drop') {
        const refused = at ? null : dropRefusal(drag.x, drag.y, kind);
        if (refused) toast.info(refused);
        else void importFiles(drag.paths, at);
      }
      if (drag.type !== 'over') kind = null;
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
