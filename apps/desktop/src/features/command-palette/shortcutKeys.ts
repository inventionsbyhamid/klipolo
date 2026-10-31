/*
 * How the editor's shortcuts read a keydown (`useEditorShortcuts`): the app's
 * modifier, the letter a ⌃⌘ key stands for, and whether an open menu or dialog has the keyboard.
 */

/**
 * A Mac, where ⌘ runs the shortcuts; Windows and Linux use Ctrl. An unknown platform (jsdom's empty
 * `navigator.platform`) counts as a Mac, the platform Klipolo ships on.
 */
export const MAC = typeof navigator === 'undefined' || !/^(Win|Linux)/.test(navigator.platform);

type Modifiers = Pick<KeyboardEvent, 'metaKey' | 'ctrlKey'>;

/** The app's modifier is held: ⌘ on a Mac, Ctrl on Windows and Linux. */
export const primaryHeld = (e: Modifiers, mac = MAC): boolean => (mac ? e.metaKey : e.ctrlKey);

/**
 * Only the other modifier is held, which runs no shortcut: on a Mac ⌃ belongs to the field (⌃K deletes to the line's
 * end, ⌃A goes to its start), and the Windows or Super key belongs to the system.
 */
export const otherAlone = (e: Modifiers, mac = MAC): boolean =>
  mac ? e.ctrlKey && !e.metaKey : e.metaKey && !e.ctrlKey;

/** The key, lower case; '' for a keydown without one (Chromium's autofill sends those, so WebView2 may). */
export function keyOf(e: { key?: string }): string {
  return typeof e.key === 'string' ? e.key.toLowerCase() : '';
}

/**
 * The letter a ⌃⌘ shortcut matches, '' for none: the key's own when it is a Latin letter, so Dvorak's ⌃⌘C stays C and
 * never runs ⌃⌘I's command from the I key it sits on; else the key it sits on (a Turkish "ı" or a Cyrillic "ш" on the
 * I key is I).
 */
export function controlLetter(e: { key?: string; code?: string }): string {
  const key = keyOf(e);
  if (/^[a-z]$/.test(key)) return key;
  const code = typeof e.code === 'string' ? /^Key([A-Z])$/.exec(e.code) : null;
  return code?.[1]?.toLowerCase() ?? '';
}

/** Keys typed into a field belong to the field. */
export function typing(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName);
}

/** An open menu, popup list or dialog owns the keyboard (its type-to-search letters, arrows and Enter). */
export const OWNS_KEYS = '[role="menu"], [role="listbox"], [role="dialog"], [role="alertdialog"]';
/**
 * A list that is part of a panel rather than a popup (the caption words) keeps its arrows,
 * Enter and Escape, but undo, redo, export and the panel keys reach the editor from it.
 */
export const PAGE_LIST = '[data-editor-keys]';

export function within(target: EventTarget | null, selector: string): boolean {
  return target instanceof Element && target.closest(selector) !== null;
}

/** Focus is in an open menu, popup list or dialog (not a panel's own list). */
export const inPopup = (target: EventTarget | null): boolean =>
  within(target, OWNS_KEYS) && !within(target, PAGE_LIST);

type ControlPress = Pick<
  KeyboardEvent,
  'key' | 'code' | 'repeat' | 'altKey' | 'shiftKey' | 'defaultPrevented' | 'target'
>;

/**
 * The command a ⌃⌘ keydown asks for (`map`: letter → command id), if any. A held key toggles once (its repeats ask for
 * nothing), and ⇧ or ⌥ with it, or a key a control already handled, ask for nothing. They work from fields and a
 * panel's own lists, but not while a menu, popup list or dialog has the keyboard: as on macOS, the panels wait until it
 * closes.
 */
export function controlCommand(e: ControlPress, map: Readonly<Record<string, string>>): string | undefined {
  if (e.repeat || e.altKey || e.shiftKey || e.defaultPrevented || inPopup(e.target)) return undefined;
  const letter = controlLetter(e);
  return letter && Object.hasOwn(map, letter) ? map[letter] : undefined;
}

type RecordPress = Pick<
  KeyboardEvent,
  'key' | 'code' | 'repeat' | 'altKey' | 'shiftKey' | 'metaKey' | 'ctrlKey' | 'defaultPrevented' | 'target'
>;

/**
 * ⇧⌘R (⇧ Ctrl R on Windows and Linux), Record your screen: R by its letter as ⌃⌘ shortcuts read it, once
 * per press (its repeats ask for nothing). Never with ⌥ or both ⌘ and Ctrl, while typing in a field, from an open menu,
 * list or dialog, or once a control handled the key.
 */
export function recordShortcut(e: RecordPress, mac = MAC): boolean {
  if (e.repeat || e.altKey || !e.shiftKey || e.defaultPrevented) return false;
  if ((e.ctrlKey && e.metaKey) || !primaryHeld(e, mac)) return false;
  return controlLetter(e) === 'r' && !typing(e.target) && !inPopup(e.target);
}
