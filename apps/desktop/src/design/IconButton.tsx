import type { LucideIcon } from 'lucide-react';
import type { ButtonHTMLAttributes, Ref } from 'react';
import { cn } from './cn';
import { Tooltip } from './Tooltip';

interface IconButtonProps extends Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  'aria-label' | 'aria-pressed'
> {
  icon: LucideIcon;
  label: string;
  /** Tooltip text when it should say more than the label, e.g. why an `aria-disabled` control is inert. */
  tooltip?: string;
  shortcut?: string;
  /** A toggle that is on: a 4 px paper dot under its icon. */
  active?: boolean;
  ref?: Ref<HTMLButtonElement>;
}

/** A 32 px icon button with 6 px corners, named by its label, with a tooltip that shows its shortcut. */
export function IconButton({
  icon: Icon,
  label,
  tooltip,
  shortcut,
  active,
  className,
  ...rest
}: IconButtonProps) {
  return (
    <Tooltip label={tooltip ?? label} shortcut={shortcut}>
      <button
        type="button"
        // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
        tabIndex={0}
        {...rest}
        aria-label={label}
        aria-pressed={active}
        className={cn('icon-btn', className)}
      >
        <Icon size={20} strokeWidth={1.8} aria-hidden />
      </button>
    </Tooltip>
  );
}
