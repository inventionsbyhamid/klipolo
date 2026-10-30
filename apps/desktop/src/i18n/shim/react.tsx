/** `@lingui/react`, English only (vite and tsconfig alias it here). */
import type { ReactNode } from 'react';
import { i18n, type I18n } from './core';
import type { Values } from './format';

/** One language: the provider only passes its children through. */
export function I18nProvider({ children }: { i18n?: I18n; children?: ReactNode }) {
  return <>{children}</>;
}

export function useLingui(): { i18n: I18n; _: I18n['_'] } {
  return { i18n, _: i18n._ };
}

/** A message by id, worded with `values`. */
export function Trans({ id, message, values }: { id: string; message?: string; values?: Values; comment?: string }) {
  return <>{i18n._(id, values, { message })}</>;
}
