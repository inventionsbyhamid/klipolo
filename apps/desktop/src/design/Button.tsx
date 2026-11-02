import type { ButtonHTMLAttributes, Ref } from 'react';
import { cn } from './cn';

type Variant = 'primary' | 'secondary' | 'ghost';
type Size = 'sm' | 'md';

/** Klipolo's buttons: red means "do it", so only a primary button is filled. */
const variants: Record<Variant, string> = {
  primary: 'btn-primary',
  secondary: 'btn-secondary',
  ghost: 'btn-ghost',
};

const sizes: Record<Size, string> = { sm: 'btn-sm', md: '' };

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  ref?: Ref<HTMLButtonElement>;
}

export function Button({
  variant = 'secondary',
  size = 'md',
  type = 'button',
  className,
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
      tabIndex={0}
      // aria-disabled keeps a control focusable (so its tooltip can say why) but inert, drawn as disabled.
      className={cn('btn', variants[variant], sizes[size], className)}
      {...rest}
    />
  );
}
