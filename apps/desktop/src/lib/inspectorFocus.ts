/** The inspector's field for a title template's words (`data-inspector-text`). */
const TEXT_FIELD = '[data-inspector-text]';

/**
 * Puts the cursor in the inspector's first text field: a title template's words are typed there, where the preview
 * shows each change at once. The inspector may only be showing from the next frame (the press selected the layer).
 */
export function focusInspectorText() {
  const focus = () => {
    const field = document.querySelector<HTMLInputElement | HTMLTextAreaElement>(TEXT_FIELD);
    if (!field) return false;
    field.focus();
    field.select();
    return true;
  };
  if (!focus()) requestAnimationFrame(() => void focus());
}
