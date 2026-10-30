/**
 * The interface language: English only. The app's code asks for the active language and the locale `Intl` formats
 * with; both are English here, and nothing switches them.
 */
import { i18n } from '@lingui/core';

export { i18n };

/** The one interface language. */
export type UiLocale = 'en';
export type AppLocale = UiLocale;
export const LOCALES: readonly AppLocale[] = ['en'];

/** The language the interface shows for a locale: English. */
export function shownLocale(_locale: UiLocale = 'en'): UiLocale {
  return 'en';
}

/** The locale `Intl` formats with. */
export function intlLocale(_locale: UiLocale = 'en'): string {
  return 'en';
}

/** The `Intl` locale of the language shown, for code outside components. */
export function activeIntlLocale(): string {
  return 'en';
}

/** Shows the interface in `locale`: English is always active, so nothing changes. */
export async function activateLocale(_locale: UiLocale): Promise<void> {}
