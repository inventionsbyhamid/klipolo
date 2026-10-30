import { cn } from './cn';

interface KbdProps {
  children: string;
  className?: string;
}

/** A keyboard shortcut such as "⌘K" (20 px tall, a 1 px line-strong edge, soft 12 px text). */
export function Kbd({ children, className }: KbdProps) {
  return <kbd className={cn(className)}>{children}</kbd>;
}
