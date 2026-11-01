import { useEffect } from 'react';
import { useRecorder } from '../../state/recorder';
import { recordShortcut } from './shortcutKeys';

/** A dialog shows: the Record dialog never opens over it, as each listens on the window and one Esc would reach both. */
const DIALOG = '[role="dialog"], [role="alertdialog"]';

/**
 * Shift Command R opens the Record dialog, from the start screen and the editor alike: never while typing
 * in a field, from an open menu or list, or over another dialog (`recordShortcut`). While a take runs it does nothing
 * (`openRecorder`).
 */
export function useRecordShortcut() {
  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if (!recordShortcut(e) || document.querySelector(DIALOG) !== null) return;
      e.preventDefault();
      void useRecorder.getState().openRecorder();
    };
    window.addEventListener('keydown', down);
    return () => window.removeEventListener('keydown', down);
  }, []);
}
