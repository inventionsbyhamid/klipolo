/** A block menu the keyboard opened is open: focus in it is still the timeline's (`useRovingFocus`). Set by
 * `BlockMenu`. */
let keyboardMenu = false;

/** Whether a block menu the keyboard opened is open (focus moved into it hasn't left the timeline). */
export function keyboardMenuOpen(): boolean {
  return keyboardMenu;
}

export function setKeyboardMenu(open: boolean) {
  keyboardMenu = open;
}
