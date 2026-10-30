/** A block menu the keyboard opened is open: focus in it is still the timeline's (`useRovingFocus`), and an item that
 * opens something else (Sync's picker) may give the keyboard back to the block when that closes. Set by `BlockMenu`. */
let keyboardMenu = false;

/** Whether a block menu the keyboard opened is open (focus moved into it hasn't left the timeline). */
export function keyboardMenuOpen(): boolean {
  return keyboardMenu;
}

export function setKeyboardMenu(open: boolean) {
  keyboardMenu = open;
}
