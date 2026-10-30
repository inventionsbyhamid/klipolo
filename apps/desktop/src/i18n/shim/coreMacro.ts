/**
 * `@lingui/core/macro`, English only (vite and tsconfig alias it here). Without the macro transform a template
 * literal's values are already in the message, so `t` returns it worded and `msg` keeps it as a descriptor.
 */
import { i18n, type MessageDescriptor } from './core';
import { choose } from './format';

type Descriptor = MessageDescriptor & { comment?: string; context?: string };

/** `t({ id, message })`, `t(descriptor)` or the tagged template `` t`…` ``: the English words. */
export function t(descriptor: Descriptor | TemplateStringsArray, ...values: unknown[]): string {
  if (Array.isArray(descriptor)) {
    const strings = descriptor as TemplateStringsArray;
    return strings.reduce((out, part, i) => out + part + (i < values.length ? String(values[i]) : ''), '');
  }
  return i18n._(descriptor as Descriptor);
}

/** A message to word later (`i18n._`), as written. */
export function msg(descriptor: Descriptor): MessageDescriptor {
  return descriptor;
}

export const defineMessage = msg;

/** `plural(count, { one: '# file', other: '# files' })`: the branch for `count`, `#` as the number. */
export function plural(value: number, options: Record<string, string>): string {
  return choose(value, options, true);
}

export const selectOrdinal = plural;

/** `select(value, { video: '…', other: '…' })`: the branch for `value`, else `other`. */
export function select(value: string, options: Record<string, string>): string {
  return choose(value, options, false);
}
