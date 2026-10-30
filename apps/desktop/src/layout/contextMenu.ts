/**
 * Inputs whose text can be typed and selected (cut, copy, paste and spelling apply); not checkboxes, sliders or
 * buttons.
 */
const TEXT_INPUTS = new Set(['', 'text', 'search', 'url', 'tel', 'email', 'password', 'number']);

/** Whether `el` is in a field whose text can be edited now: enabled, not read-only, and holding text. */
function editableText(el: Element): boolean {
  const field = el.closest('input, textarea');
  if (field instanceof HTMLInputElement)
    return !field.disabled && !field.readOnly && TEXT_INPUTS.has(field.getAttribute('type')?.toLowerCase() ?? '');
  if (field instanceof HTMLTextAreaElement) return !field.disabled && !field.readOnly;
  // The nearest `contenteditable` decides: an island marked "false" inside an editor isn't editable.
  const value = el.closest('[contenteditable]')?.getAttribute('contenteditable')?.toLowerCase();
  return value === '' || value === 'true' || value === 'plaintext-only';
}

/**
 * Hides the webview's own right-click menu (Reload, Back, Inspect Element) everywhere except fields whose text can be
 * edited. Returns the function that turns it back on.
 */
export function suppressContextMenu(target: Document = document): () => void {
  const onMenu = (event: Event) => {
    const el = event.target instanceof Element ? event.target : null;
    if (!el || !editableText(el)) event.preventDefault();
  };
  target.addEventListener('contextmenu', onMenu);
  return () => target.removeEventListener('contextmenu', onMenu);
}
