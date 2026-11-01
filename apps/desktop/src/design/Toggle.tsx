import type { ButtonHTMLAttributes, Ref } from 'react';
import { cn } from './cn';
import './toggle.css';

export interface ToggleProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'onChange' | 'role' | 'aria-checked' | 'type'> {
  checked: boolean;
  onChange: (checked: boolean) => void;
  ref?: Ref<HTMLButtonElement>;
}

/**
 * A toggle for a setting that applies at once: a 36 x 20 track, raised with an edge and a soft knob when
 * off, paper with an on-paper knob when on. A switch: Space or a click flips it, and a `<label htmlFor>` beside it
 * names it (a click on the label flips it too).
 */
export function Toggle({ checked, onChange, className, onClick, ...rest }: ToggleProps) {
  return (
    <button
      type="button"
      // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
      tabIndex={0}
      {...rest}
      role="switch"
      aria-checked={checked}
      className={cn('toggle', className)}
      onClick={(e) => {
        onClick?.(e);
        if (!e.defaultPrevented) onChange(!checked);
      }}
    />
  );
}
