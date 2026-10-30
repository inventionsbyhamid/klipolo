/**
 * English message formatting for the lingui shim: `{name}` values, and `{name, select, …}` and `{name, plural, …}`
 * (exact `=N` branches first, then the English plural category, `#` as the number). Lenient on purpose: a placeholder
 * with no value, or braces that aren't a message argument (a file name with `{` in it), stay as written.
 */

export type Values = Record<string, unknown>;

const PLURALS = new Intl.PluralRules('en');
const NUMBER = new Intl.NumberFormat('en');

/** A number as English shows it ("1,200"). */
export function formatNumber(value: number): string {
  return NUMBER.format(value);
}

/** The index of the `}` closing the `{` at `open`, or -1. */
function closing(text: string, open: number): number {
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    if (text[i] === '{') depth++;
    else if (text[i] === '}' && --depth === 0) return i;
  }
  return -1;
}

/** `select` or `plural` branches (`key {text} key {text}`), or null when they don't parse. */
function branches(body: string): Map<string, string> | null {
  const out = new Map<string, string>();
  let i = 0;
  while (i < body.length) {
    while (i < body.length && /\s/.test(body[i])) i++;
    if (i >= body.length) break;
    const keyStart = i;
    while (i < body.length && !/[\s{]/.test(body[i])) i++;
    const key = body.slice(keyStart, i);
    while (i < body.length && /\s/.test(body[i])) i++;
    if (!key || body[i] !== '{') return null;
    const end = closing(body, i);
    if (end < 0) return null;
    out.set(key, body.slice(i + 1, end));
    i = end + 1;
  }
  return out.size > 0 ? out : null;
}

function stringOf(value: unknown): string {
  return typeof value === 'number' ? formatNumber(value) : String(value);
}

/** One argument (`name`, `name, select, …` or `name, plural, …`), or null to keep it as written. */
function argument(inner: string, values: Values, count: number | null): string | null {
  const match = /^\s*([A-Za-z_]\w*)\s*(?:,\s*(select|plural)\s*,([\s\S]*))?$/.exec(inner);
  if (!match) return null;
  const [, name, kind, rest] = match;
  if (!Object.hasOwn(values, name)) return null;
  const value = values[name];
  if (!kind) return stringOf(value);
  const options = branches(rest);
  if (!options) return null;
  if (kind === 'select') {
    const picked = options.get(String(value)) ?? options.get('other');
    return picked === undefined ? null : format(picked, values, count);
  }
  const n = Number(value);
  const picked = options.get(`=${n}`) ?? options.get(PLURALS.select(n)) ?? options.get('other');
  return picked === undefined ? null : format(picked, values, n);
}

/** `message` with `values`; `count` is the number `#` stands for inside a plural branch. */
function format(message: string, values: Values, count: number | null): string {
  let out = '';
  let i = 0;
  while (i < message.length) {
    const c = message[i];
    if (c === '#' && count !== null) {
      out += formatNumber(count);
      i++;
    } else if (c === '{') {
      const end = closing(message, i);
      const worded = end < 0 ? null : argument(message.slice(i + 1, end), values, count);
      if (worded === null) {
        out += c;
        i++;
      } else {
        out += worded;
        i = end + 1;
      }
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

export function formatMessage(message: string, values: Values = {}): string {
  return format(message, values, null);
}

/** The plural and select macros' choice: an exact branch (`1` or `=1`), then the English category, then `other`. */
export function choose(value: number | string, options: Record<string, string>, plural: boolean): string {
  if (!plural) return options[String(value)] ?? options.other ?? '';
  const n = Number(value);
  const branch = options[`=${n}`] ?? options[String(n)] ?? options[PLURALS.select(n)] ?? options.other ?? '';
  return branch.replaceAll('#', formatNumber(n));
}
