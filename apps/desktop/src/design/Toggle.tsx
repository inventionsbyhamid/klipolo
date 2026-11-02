import { Switch } from 'radix-ui';
import type { ComponentProps } from 'react';
import { cn } from './cn';

/**
 * A toggle for a setting that applies at once: `role="switch"`, named by its label (`id` and a `<label
 * htmlFor>`, or `aria-label`). A click on it or its label, or Space, flips it. Off is a sunken track with a soft knob;
 * on is signal yellow with a dark knob.
 */
export function Toggle({ className, ...props }: ComponentProps<typeof Switch.Root>) {
  return (
    <Switch.Root
      // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
      tabIndex={0}
      className={cn('toggle', className)}
      {...props}
    >
      <Switch.Thumb className="toggle-knob" />
    </Switch.Root>
  );
}
