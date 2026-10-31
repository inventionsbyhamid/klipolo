import { useLayoutEffect, useRef, type FocusEvent } from 'react';

/**
 * Puts the keyboard in the chat box, where the person types next. When it is disabled (no key or code), the chat
 * panel's body takes it instead (it is focusable, and there even when the conversation is empty), so focus never falls
 * to the page.
 */
export function focusComposer() {
  const box = document.querySelector<HTMLTextAreaElement>('textarea[data-chat-composer]');
  if (!box) return;
  if (!box.disabled) return box.focus();
  document.querySelector<HTMLElement>('[data-chat-body-focus]')?.focus();
}

/**
 * For a part of the panel that goes away while it may have the keyboard (a proposal card collapsing): when `present`
 * turns false with focus inside it, focus goes back to the chat box (`focusComposer`), never to the page. Focus that
 * left it for something else is left alone. Spread `focusProps` on its element.
 */
export function useReturnFocus(present: boolean) {
  /** Focus is inside now (or was when its focused control went away). */
  const inside = useRef(false);
  useLayoutEffect(() => {
    if (present || !inside.current) return;
    inside.current = false;
    const now = document.activeElement;
    if (now === null || now === document.body) focusComposer();
  }, [present]);
  return {
    onFocus: () => {
      inside.current = true;
    },
    onBlur: (e: FocusEvent<HTMLElement>) => {
      const box = e.currentTarget;
      const left = e.target;
      if (e.relatedTarget instanceof Node && box.contains(e.relatedTarget)) return;
      // Checked once the event is over: a control still focused only lost the window's focus; one still in the page
      // gave focus to something else; one that is gone was removed as the part went, which the effect above handles.
      queueMicrotask(() => {
        if (left.isConnected && document.activeElement !== left) inside.current = false;
      });
    },
  };
}
