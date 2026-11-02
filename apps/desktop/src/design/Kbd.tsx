import { cn } from './cn';

interface KbdProps {
  children: string;
  className?: string;
}

/**
 * A keyboard shortcut such as "⌘K": 18 px tall, 11 px text in the colour around it, a 1 px edge of that colour at
 * 30 %.
 */
export function Kbd({ children, className }: KbdProps) {
  return <kbd className={cn(className)}>{children}</kbd>;
}
