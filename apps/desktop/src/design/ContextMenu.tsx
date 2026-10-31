import { ContextMenu } from 'radix-ui';
import { useId, type ComponentProps, type ReactNode } from 'react';
import { menuItemClass } from './Menu';

/**
 * A menu opened on an element (right-click, Shift+F10, the Menu key), styled like `Menu`. Its content mounts only
 * while open. Where focus goes when it closes is the caller's (`onCloseAutoFocus`): back to what it was opened on
 * after keyboard use, left alone after a click, as `Menu` does.
 */
export const ContextMenuRoot = ContextMenu.Root;
export const ContextMenuTrigger = ContextMenu.Trigger;

export function ContextMenuContent({
  label,
  onCloseAutoFocus,
  children,
}: {
  /** The menu's name for screen readers. */
  label?: string;
  onCloseAutoFocus?: ComponentProps<typeof ContextMenu.Content>['onCloseAutoFocus'];
  children: ReactNode;
}) {
  return (
    <ContextMenu.Portal>
      <ContextMenu.Content
        aria-label={label}
        onCloseAutoFocus={onCloseAutoFocus}
        className="menu"
      >
        {children}
      </ContextMenu.Content>
    </ContextMenu.Portal>
  );
}

export function ContextMenuItem({
  children,
  shortcut,
  onSelect,
  disabled,
  note,
}: {
  children: ReactNode;
  /** Its keyboard shortcut ("⌘C"), shown at the end. */
  shortcut?: string;
  onSelect: () => void;
  disabled?: boolean;
  /** What to know about it, such as why it is off: its tooltip, and its description for screen readers. */
  note?: string;
}) {
  const noteId = useId();
  return (
    <ContextMenu.Item
      className={menuItemClass}
      onSelect={onSelect}
      disabled={disabled}
      title={note}
      aria-describedby={note ? noteId : undefined}
    >
      <span className="menu-item-text">{children}</span>
      {shortcut && <span className="menu-key">{shortcut}</span>}
      {note && (
        <span id={noteId} hidden>
          {note}
        </span>
      )}
    </ContextMenu.Item>
  );
}

export function ContextMenuSeparator() {
  return <ContextMenu.Separator className="menu-sep" />;
}
