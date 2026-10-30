import type { ReactNode } from 'react';
import { intlLocale, type UiLocale } from '.';

/** Gives every component the active language (English only: its children render as they are). */
export function LocaleProvider({ children }: { children: ReactNode }) {
  return <>{children}</>;
}

/** The language the interface shows. */
export function useLocale(): UiLocale {
  return 'en';
}

/** The locale to hand `Intl` (and the `lib/format` helpers) for the language the interface shows. */
export function useIntlLocale(): string {
  return intlLocale(useLocale());
}
