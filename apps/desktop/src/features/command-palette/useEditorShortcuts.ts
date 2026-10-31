import { useEffect, useEffectEvent } from 'react';
import { flushBursts } from '../../state/bursts';
import { useRange } from '../../state/range';
import { useSelection } from '../../state/selection';
import type { RunOptions } from './command';
import { inPopup, keyOf, otherAlone, OWNS_KEYS, PAGE_LIST, primaryHeld, within } from './shortcutKeys';
import { markRange } from '../timeline/commands';
import { useCommands } from './useCommands';

/** Keys typed into a field belong to the field. */
function typing(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName);
}

/** Undo, redo and export reach the editor from a panel's own list (`PAGE_LIST`). */
const FROM_PAGE_LIST = new Set(['z', 'e']);
/** Space presses the focused control; it only plays the video when no control has focus. */
const PRESSES =
  'button, a[href], summary, [role="button"], [role="checkbox"], [role="switch"], [role="radio"], [role="tab"]';

/** ⌘C, ⌘X, ⌘V and ⌘D leave the system's clipboard and keys alone when there is nothing of ours to act on. */
const OPTIONAL = new Set(['copy', 'cut', 'paste', 'duplicate', 'paste-effects']);

/**
 * Editor keyboard shortcuts, with ⌘ on a Mac and Ctrl on Windows and Linux; ⌃ alone on a Mac (the field's
 * own keys, such as ⌃K) and the Windows or Super key alone run none (`otherAlone`). ⌘K puts the cursor in the chat;
 * ⌘Z undo; ⇧⌘Z redo; ⌘O add files; ⌘E export; S split; Space play/pause; J / L play backward / forward (pressed
 * again while playing that way: double speed); K pause; with K held, J / L one frame; Delete or Backspace delete (the
 * selected time range, else the selected clip or layer); I / O the time range's start or end at the playhead; ← / →
 * one frame (⇧ one second); ↑ / ↓ the previous or next edit; Home / End the start or end; ⌘= / ⌘− timeline zoom; ⇧Z
 * fit the timeline; 1 / 2 / 3 the format the preview shows; N snapping on or off; Escape clears the selection. With a
 * clip or layer selected: Q / W trim its start or end to the playhead; , and . move it a frame (⇧ ten) or a clip a
 * place; ⌘C ⌘X ⌘V ⌘D copy, cut, paste, duplicate; ⌥⌘V paste effects; ⌥↑ / ⌥↓ the track above or below.
 * (On a focused timeline block the arrows, Home, End, Enter and Space move between blocks: `useRovingFocus`.)
 * Everything but ⌘K is ignored while typing in a field; everything while a menu, list or dialog is open, and when a
 * focused control already handled the key.
 */
export function useEditorShortcuts() {
  const commands = useCommands();

  /** `kHeld`: K is down, so J and L step a frame instead of playing (as in other editors). */
  const onKey = useEffectEvent((e: KeyboardEvent, kHeld: boolean) => {
    const key = keyOf(e);
    /** Runs command `id` if it can act now; true when it ran. */
    const run = (id: string, options?: RunOptions) => {
      const command = commands.find((c) => c.id === id);
      // A toolbar change still settling makes Undo possible even before it reaches the history.
      const settling = (id === 'undo' || id === 'redo') && flushBursts();
      if (!command?.enabled && !(command && settling)) return false;
      command.run(options);
      return true;
    };
    if (e.ctrlKey && e.metaKey) return;
    // On a Mac, ⌃ alone used to run the ⌘ shortcuts: ⌃K took kill-line from a field, and ⌃Z, ⌃I, ⌃E and ⌃D undid,
    // imported, exported and duplicated.
    if (otherAlone(e)) return;
    const mod = primaryHeld(e);
    // ⌘K works from a field too, and waits for an open menu or dialog.
    if (mod && key === 'k' && !e.shiftKey && !e.altKey) {
      if (e.defaultPrevented || inPopup(e.target)) return;
      e.preventDefault();
      run('focus-chat');
      return;
    }
    const fromList = mod && FROM_PAGE_LIST.has(key) && within(e.target, PAGE_LIST);
    if (e.defaultPrevented || typing(e.target) || (within(e.target, OWNS_KEYS) && !fromList)) return;

    // ⌥⌘V and ⌥↑ / ⌥↓ (`code` for V: ⌥ turns it into "√" on a Mac).
    if (e.altKey) {
      if (mod && !e.shiftKey && e.code === 'KeyV') {
        if (run('paste-effects')) e.preventDefault();
      } else if (!mod && !e.shiftKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
        e.preventDefault();
        run(e.key === 'ArrowUp' ? 'track-up' : 'track-down');
      }
      return;
    }
    const withMod: Record<string, string> = {
      z: e.shiftKey ? 'redo' : 'undo',
      o: 'import',
      e: 'export',
      '=': 'zoom-in',
      '+': 'zoom-in',
      '-': 'zoom-out',
      c: 'copy',
      x: 'cut',
      v: 'paste',
      d: 'duplicate',
    };
    if (mod) {
      const id = withMod[key];
      if (!id) return;
      if (OPTIONAL.has(id)) {
        if (run(id)) e.preventDefault();
        return;
      }
      e.preventDefault();
      run(id);
    } else if (e.key === 'Escape') {
      useSelection.getState().select(null);
      useRange.getState().clear();
    } else if (key === 'k' && !e.shiftKey) {
      run('pause');
    } else if ((key === 'j' || key === 'l') && !e.shiftKey) {
      const forward = key === 'l';
      if (kHeld) {
        e.preventDefault();
        run(forward ? 'frame-forward' : 'frame-back');
      } else if (!e.repeat) {
        // Holding the key doesn't keep speeding up.
        run(forward ? 'play-forward' : 'play-backward');
      }
    } else if (key === 's' && !e.shiftKey) {
      run('split');
    } else if ((key === 'q' || key === 'w') && !e.shiftKey) {
      run(key === 'q' ? 'trim-start' : 'trim-end', { repeat: e.repeat });
    } else if (e.code === 'Comma' || e.code === 'Period') {
      // `code`: ⇧ turns them into < and >. ⇧ moves a layer ten frames.
      e.preventDefault();
      run(e.code === 'Comma' ? 'nudge-back' : 'nudge-forward', { large: e.shiftKey, repeat: e.repeat });
    } else if (key === 'z' && e.shiftKey) {
      run('zoom-fit');
    } else if (key === 'n' && !e.shiftKey) {
      run('snapping');
    } else if ((key === 'i' || key === 'o') && !e.shiftKey) {
      markRange(key === 'i' ? 'in' : 'out');
    } else if (['1', '2', '3'].includes(e.key) && !e.shiftKey) {
      run(`format-${e.key}`);
    } else if (e.key === ' ' && !within(e.target, PRESSES)) {
      e.preventDefault();
      run('play');
    } else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      run('delete');
    } else if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault();
      run(e.key === 'ArrowLeft' ? 'frame-back' : 'frame-forward', { large: e.shiftKey });
    } else if ((e.key === 'ArrowUp' || e.key === 'ArrowDown') && !e.shiftKey) {
      e.preventDefault();
      run(e.key === 'ArrowUp' ? 'prev-edit' : 'next-edit');
    } else if ((e.key === 'Home' || e.key === 'End') && !e.shiftKey) {
      e.preventDefault();
      run(e.key === 'Home' ? 'go-start' : 'go-end');
    }
  });

  useEffect(() => {
    let kHeld = false;
    const down = (e: KeyboardEvent) => {
      if (keyOf(e) === 'k' && !e.metaKey && !e.ctrlKey && !e.altKey) kHeld = true;
      onKey(e, kHeld);
    };
    const up = (e: KeyboardEvent) => {
      if (keyOf(e) === 'k') kHeld = false;
    };
    // K released in another window never reaches this one.
    const blur = () => {
      kHeld = false;
    };
    window.addEventListener('keydown', down);
    window.addEventListener('keyup', up);
    window.addEventListener('blur', blur);
    return () => {
      window.removeEventListener('keydown', down);
      window.removeEventListener('keyup', up);
      window.removeEventListener('blur', blur);
    };
  }, []);
}
