import { Check } from 'lucide-react';
import { DropdownMenu } from 'radix-ui';
import { createContext, use, useId, useState, type ComponentProps, type ReactNode } from 'react';
import { cn } from './cn';

/** Whether the open menu has been used with the pointer (opened, clicked in, or dismissed by clicking away). */
function pointerTracker() {
  let used = false;
  return { set: (value: boolean) => void (used = value), get: () => used };
}
const PointerUse = createContext<ReturnType<typeof pointerTracker> | null>(null);

/**
 * A dropdown menu. Closing it puts focus back on its trigger only when it was used from the keyboard: after a mouse
 * user closes it, a focused trigger would turn the next Space into "reopen the menu" instead of play.
 */
export function Menu(props: ComponentProps<typeof DropdownMenu.Root>) {
  const [tracker] = useState(pointerTracker);
  return (
    <PointerUse value={tracker}>
      <DropdownMenu.Root {...props} />
    </PointerUse>
  );
}

/**
 * Inside a `Menu`'s content: whether the menu was used with the pointer (opened, clicked in, dismissed by clicking
 * away), as its own focus return decides. An item that opens something else gives it a way back to the trigger only
 * after keyboard use.
 */
export function useMenuPointerUsed(): () => boolean {
  const tracker = use(PointerUse);
  return () => tracker?.get() ?? false;
}

export function MenuTrigger({
  onPointerDown,
  onKeyDown,
  ...props
}: ComponentProps<typeof DropdownMenu.Trigger>) {
  const tracker = use(PointerUse);
  return (
    <DropdownMenu.Trigger
      // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on.
      tabIndex={0}
      {...props}
      onPointerDown={(e) => {
        tracker?.set(true);
        onPointerDown?.(e);
      }}
      onKeyDown={(e) => {
        tracker?.set(false);
        onKeyDown?.(e);
      }}
    />
  );
}

// Klipolo's menu rows: 32 px, the highlighted (pointer or keyboard) row on the hover fill; icons inherit.
export const menuItemClass = 'menu-item';

export function MenuContent({
  children,
  align = 'start',
  onCloseAutoFocus,
}: {
  children: ReactNode;
  align?: 'start' | 'end';
  /** As the menu closes, before focus goes back to its trigger: `preventDefault` leaves focus where an item put it. */
  onCloseAutoFocus?: (e: Event) => void;
}) {
  const tracker = use(PointerUse);
  return (
    <DropdownMenu.Portal>
      <DropdownMenu.Content
        align={align}
        sideOffset={6}
        onPointerDown={() => tracker?.set(true)}
        onPointerDownOutside={() => tracker?.set(true)}
        onCloseAutoFocus={(e) => {
          onCloseAutoFocus?.(e);
          if (tracker?.get()) e.preventDefault();
        }}
        className="menu"
      >
        {children}
      </DropdownMenu.Content>
    </DropdownMenu.Portal>
  );
}

export function MenuItem({
  children,
  onSelect,
  disabled,
  describedBy,
}: {
  children: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
  /** Id of a `MenuNote` that explains the item, e.g. why it is disabled. */
  describedBy?: string;
}) {
  return (
    <DropdownMenu.Item
      className={menuItemClass}
      onSelect={onSelect}
      disabled={disabled}
      aria-describedby={describedBy}
    >
      {children}
    </DropdownMenu.Item>
  );
}

export function MenuCheckboxItem({
  children,
  checked,
  onCheckedChange,
  disabled,
  describedBy,
}: {
  children: ReactNode;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean;
  /** Id of a `MenuNote` that explains the item, e.g. why it is disabled. */
  describedBy?: string;
}) {
  return (
    <DropdownMenu.CheckboxItem
      className={cn(menuItemClass, 'has-check')}
      checked={checked}
      onCheckedChange={onCheckedChange}
      disabled={disabled}
      aria-describedby={describedBy}
      onSelect={(event) => event.preventDefault()}
    >
      <DropdownMenu.ItemIndicator className="menu-check">
        <Check size={14} aria-hidden />
      </DropdownMenu.ItemIndicator>
      {children}
    </DropdownMenu.CheckboxItem>
  );
}

/**
 * One choice out of several, e.g. a speed. `label` titles the group when a menu holds more than one. Picking the
 * current value again does nothing, so it never makes an empty undo step.
 */
export function MenuRadioGroup<T extends string>({
  label,
  value,
  onValueChange,
  children,
}: {
  label?: string;
  value: T;
  onValueChange: (value: T) => void;
  children: ReactNode;
}) {
  const labelId = useId();
  return (
    <DropdownMenu.RadioGroup
      value={value}
      onValueChange={(next) => next !== value && onValueChange(next as T)}
      aria-labelledby={label ? labelId : undefined}
    >
      {label && (
        <DropdownMenu.Label id={labelId} className="menu-label">
          {label}
        </DropdownMenu.Label>
      )}
      {children}
    </DropdownMenu.RadioGroup>
  );
}

export function MenuRadioItem({
  value,
  children,
  disabled,
  describedBy,
}: {
  value: string;
  children: ReactNode;
  disabled?: boolean;
  /** Id of a `MenuNote` that explains the item, e.g. why it is disabled. */
  describedBy?: string;
}) {
  return (
    <DropdownMenu.RadioItem
      className={cn(menuItemClass, 'has-check')}
      value={value}
      disabled={disabled}
      aria-describedby={describedBy}
    >
      <DropdownMenu.ItemIndicator className="menu-check">
        <Check size={14} aria-hidden />
      </DropdownMenu.ItemIndicator>
      {children}
    </DropdownMenu.RadioItem>
  );
}

export function MenuSeparator() {
  return <DropdownMenu.Separator className="menu-sep" />;
}

/** A short, non-interactive line of help inside a menu, e.g. why an item is disabled. */
export function MenuNote({ id, children }: { id?: string; children: ReactNode }) {
  return (
    <p id={id} className="menu-note">
      {children}
    </p>
  );
}
