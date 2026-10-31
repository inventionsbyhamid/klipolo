import { clsx, type ClassValue } from 'clsx';

/**
 * Joins class names. Additive classes only: there is no tailwind-merge, so two classes that set the same property
 * resolve by stylesheet order, not by argument order. Pick classes conditionally instead of overriding them.
 */
export function cn(...values: ClassValue[]): string {
  return clsx(values);
}
