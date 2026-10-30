import { Popover as P } from 'radix-ui';
import type { ComponentProps } from 'react';
import { cn } from './cn';

export const Popover = P.Root;
export const PopoverTrigger = P.Trigger;

export function PopoverContent({ className, ...props }: ComponentProps<typeof P.Content>) {
  return (
    <P.Portal>
      <P.Content
        sideOffset={4}
        collisionPadding={8}
        className={cn('popover', className)}
        {...props}
      />
    </P.Portal>
  );
}
