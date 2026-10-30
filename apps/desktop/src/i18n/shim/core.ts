/**
 * `@lingui/core`, English only (vite and tsconfig alias it here). `i18n._` words a message by id with its values, the
 * way lingui's compiled catalogue would for English: the message is the English text.
 */
import { formatMessage, formatNumber, type Values } from './format';

export interface MessageDescriptor {
  id: string;
  message?: string;
  comment?: string;
  values?: Values;
}

export type Messages = Record<string, string>;

export interface I18n {
  locale: string;
  locales: string[];
  _(descriptor: MessageDescriptor): string;
  _(id: string, values?: Values, options?: { message?: string }): string;
  t: I18n['_'];
  number(value: number, options?: Intl.NumberFormatOptions): string;
  date(value: Date | number | string, options?: Intl.DateTimeFormatOptions): string;
  /** Nothing changes language: listeners are never called. */
  on(event: string, listener: (...args: unknown[]) => void): () => void;
  activate(locale: string): void;
  loadAndActivate(options: { locale: string; locales?: string[]; messages?: Messages }): void;
}

function word(idOrDescriptor: string | MessageDescriptor, values?: Values, options?: { message?: string }): string {
  if (typeof idOrDescriptor === 'string')
    return formatMessage(options?.message ?? idOrDescriptor, values ?? {});
  return formatMessage(idOrDescriptor.message ?? idOrDescriptor.id, idOrDescriptor.values ?? {});
}

export function setupI18n(): I18n {
  return {
    locale: 'en',
    locales: ['en'],
    _: word as I18n['_'],
    t: word as I18n['_'],
    number: (value, options) =>
      options ? new Intl.NumberFormat('en', options).format(value) : formatNumber(value),
    date: (value, options) => new Intl.DateTimeFormat('en', options).format(new Date(value)),
    on: () => () => undefined,
    activate: () => undefined,
    loadAndActivate: () => undefined,
  };
}

export const i18n = setupI18n();
