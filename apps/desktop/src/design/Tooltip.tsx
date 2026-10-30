import { Slot, Tooltip as T } from 'radix-ui';
import { useId, useState, type ComponentProps, type ReactElement, type ReactNode } from 'react';
import { Kbd } from './Kbd';

export function TooltipProvider({ children }: { children: ReactNode }) {
  return (
    <T.Provider delayDuration={400} skipDelayDuration={200}>
      {children}
    </T.Provider>
  );
}

interface TooltipProps {
  label: string;
  shortcut?: string;
  side?: 'top' | 'right' | 'bottom' | 'left';
  /**
   * The label is also the trigger's lasting description (`aria-describedby`), such as why an inert control can't act.
   * A press closes the tooltip, but not this; and the open tooltip doesn't add itself to it, so it is read once.
   */
  describes?: boolean;
  children: ReactElement;
}

/**
 * Passes the trigger's props on to its child; with a `note`, described by that alone, not also by the open tooltip.
 * Always there, so a control that starts or stops describing itself (at eight effects) stays the same element.
 */
function TriggerChild({ note, ...props }: ComponentProps<typeof Slot.Root> & { note?: string }) {
  return note ? <Slot.Root {...props} aria-describedby={note} /> : <Slot.Root {...props} />;
}

export function Tooltip({ label, shortcut, side = 'bottom', describes = false, children }: TooltipProps) {
  const [open, setOpen] = useState(false);
  const noteId = useId();
  return (
    <T.Root open={open} onOpenChange={setOpen}>
      {/* A press hides the tooltip. Radix does that itself unless the trigger prevents the press's default, as a
          menu trigger does, which left the tooltip showing under the open menu. */}
      <T.Trigger asChild onPointerDown={() => setOpen(false)}>
        <TriggerChild note={describes ? noteId : undefined}>{children}</TriggerChild>
      </T.Trigger>
      {describes && (
        <span id={noteId} hidden>
          {label}
        </span>
      )}
      <T.Portal>
        <T.Content
          side={side}
          sideOffset={6}
          className="tooltip"
        >
          {label}
          {shortcut && <Kbd>{shortcut}</Kbd>}
        </T.Content>
      </T.Portal>
    </T.Root>
  );
}
