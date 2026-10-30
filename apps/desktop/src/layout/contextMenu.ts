/** Text fields keep their menu (cut, copy, paste, spelling). */
const TEXT_FIELDS = 'input, textarea, [contenteditable=""], [contenteditable="true"]';

/**
 * Hides the webview's own right-click menu (Reload, Back, Inspect Element) everywhere except text fields. Returns the
 * function that turns it back on.
 */
export function suppressContextMenu(target: Document = document): () => void {
  const onMenu = (event: Event) => {
    const el = event.target instanceof Element ? event.target : null;
    if (!el?.closest(TEXT_FIELDS)) event.preventDefault();
  };
  target.addEventListener('contextmenu', onMenu);
  return () => target.removeEventListener('contextmenu', onMenu);
}
