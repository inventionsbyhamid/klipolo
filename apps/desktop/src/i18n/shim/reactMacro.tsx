/** `@lingui/react/macro`, English only (vite and tsconfig alias it here). */
import type { ReactNode } from 'react';
import { i18n, type I18n } from './core';
import { t } from './coreMacro';

/** The macro's `<Trans>`: its children are the English words (values included). */
export function Trans({ children }: { id?: string; comment?: string; context?: string; children?: ReactNode }) {
  return <>{children}</>;
}

export function useLingui(): { t: typeof t; i18n: I18n; _: I18n['_'] } {
  return { t, i18n, _: i18n._ };
}
