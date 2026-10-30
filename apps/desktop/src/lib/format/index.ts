/**
 * Numbers, sizes, durations in words and dates in the active language, through `Intl`. Each takes
 * the locale to format in: components get it from `useIntlLocale()`, so they re-render when the language changes.
 *
 * Clock times (`lib/time`: 1:05, 1:05.3) and timecodes (HH:MM:SS:FF) are not formatted here: they read the same in
 * every language and stay untranslated.
 */

const formatters = new Map<
  string,
  Intl.NumberFormat | Intl.DateTimeFormat | Intl.ListFormat | Intl.RelativeTimeFormat
>();

/** One formatter per locale and options, made on first use: building an `Intl` formatter is the costly part. */
function cached<
  T extends Intl.NumberFormat | Intl.DateTimeFormat | Intl.ListFormat | Intl.RelativeTimeFormat,
>(key: string, make: () => T): T {
  let formatter = formatters.get(key) as T | undefined;
  if (!formatter) {
    formatter = make();
    formatters.set(key, formatter);
  }
  return formatter;
}

function numberFormat(locale: string, options: Intl.NumberFormatOptions): Intl.NumberFormat {
  return cached(`n|${locale}|${JSON.stringify(options)}`, () => new Intl.NumberFormat(locale, options));
}

export function formatNumber(locale: string, value: number, options: Intl.NumberFormatOptions = {}): string {
  return numberFormat(locale, options).format(value);
}

/** A percentage of 0 to 1: 0.42 → "42 %" in German, "42%" in English. */
export function formatPercent(locale: string, fraction: number): string {
  return numberFormat(locale, { style: 'percent', maximumFractionDigits: 0 }).format(fraction);
}

/**
 * An estimated file size: "140 MB", "3.2 GB" (decimal units, as Finder shows them). Megabytes round up, to tens
 * from 100 MB, and never show under 1 MB.
 */
export function formatFileSize(locale: string, bytes: number): string {
  if (bytes >= 1e9) {
    return numberFormat(locale, {
      style: 'unit',
      unit: 'gigabyte',
      minimumFractionDigits: 1,
      maximumFractionDigits: 1,
    }).format(bytes / 1e9);
  }
  return numberFormat(locale, { style: 'unit', unit: 'megabyte', maximumFractionDigits: 0 }).format(
    shownMegabytes(bytes),
  );
}

/** The megabytes `formatFileSize` shows for a size under 1 GB. */
function shownMegabytes(bytes: number): number {
  const mb = bytes / 1e6;
  return mb >= 100 ? Math.ceil(mb / 10) * 10 : Math.max(1, Math.ceil(mb));
}

/**
 * A download's bytes so far beside its total, both in the total's unit as `formatFileSize` shows it: `{ done: '1.1',
 * total: '3.0 GB' }` or `{ done: '120', total: '310 MB' }`, for "1.1 of 3.0 GB". The done figure rounds down and stays
 * a step under the total's until all of it is there, so "2.7 of 2.7 GB" never shows with some still to come.
 */
export function formatSizeOf(locale: string, done: number, total: number): { done: string; total: string } {
  const gb = total >= 1e9;
  // In tenths of a gigabyte, or in megabytes.
  const step = gb ? 1e8 : 1e6;
  const whole = gb ? Math.round(total / step) : shownMegabytes(total);
  const steps = done >= total ? whole : Math.max(0, Math.min(Math.floor(done / step), whole - 1));
  const digits = gb ? 1 : 0;
  return {
    done: numberFormat(locale, { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(
      gb ? steps / 10 : steps,
    ),
    total: formatFileSize(locale, total),
  };
}

/**
 * A download's exact size, for small files (a font family is 0.06 to 6 MB): "60 kB", "2.9 MB", "15 MB" (decimal units,
 * as Finder shows them). Under 1 MB in whole kilobytes (at least 1), one decimal under 10 MB, whole megabytes above.
 */
export function formatDownloadSize(locale: string, bytes: number): string {
  if (bytes < 1e6) {
    const kb = Math.max(1, Math.round(bytes / 1e3));
    return numberFormat(locale, { style: 'unit', unit: 'kilobyte', maximumFractionDigits: 0 }).format(kb);
  }
  const mb = bytes / 1e6;
  const digits = mb < 10 ? 1 : 0;
  return numberFormat(locale, { style: 'unit', unit: 'megabyte', maximumFractionDigits: digits }).format(mb);
}

/**
 * A file's size as it is, as Finder shows it (decimal units): "512 bytes", "24 kB", "3.2 MB", "1.5 GB". Unlike
 * `formatFileSize` (an estimate), a small file isn't rounded up to a megabyte.
 */
export function formatBytes(locale: string, bytes: number): string {
  const unit = (unit: string, value: number, digits: number) =>
    numberFormat(locale, { style: 'unit', unit, maximumFractionDigits: digits }).format(value);
  if (bytes >= 1e9) return unit('gigabyte', bytes / 1e9, 1);
  if (bytes >= 1e6) return unit('megabyte', bytes / 1e6, 1);
  if (bytes >= 1e3) return unit('kilobyte', bytes / 1e3, 0);
  return numberFormat(locale, { style: 'unit', unit: 'byte', unitDisplay: 'long' }).format(bytes);
}

/**
 * Seconds with the SI symbol, which every interface language uses: "1.0 s" ("1,0 s" in German) with one decimal, or
 * `short` without a trailing zero: "1 s", "0.5 s".
 */
export function formatSeconds(locale: string, seconds: number, digits: 'one' | 'short' = 'one'): string {
  const options = { minimumFractionDigits: digits === 'one' ? 1 : 0, maximumFractionDigits: 1 };
  return `${numberFormat(locale, options).format(seconds)} s`;
}

/**
 * Pixels with their symbol, which every interface language uses (like seconds): "12 px", "1.5 px" ("1,5 px" in
 * German), with at most `digits` decimals.
 */
export function formatPixels(locale: string, px: number, digits = 0): string {
  // eslint-disable-next-line no-restricted-syntax -- "px" is a unit symbol, not a word
  return `${numberFormat(locale, { maximumFractionDigits: digits }).format(px)} px`;
}

/**
 * A length in words, to the nearest second: "1 minute, 5 seconds", "2 hours" (screen readers, summaries). Zero
 * units are left out; anything under half a second is "0 seconds".
 */
export function formatDurationWords(locale: string, us: number): string {
  const total = Math.max(0, Math.round(us / 1_000_000));
  const parts: [Intl.NumberFormatOptions['unit'], number][] = [
    ['hour', Math.floor(total / 3600)],
    ['minute', Math.floor((total % 3600) / 60)],
    ['second', total % 60],
  ];
  const words = parts
    .filter(([, n]) => n > 0)
    .map(([unit, n]) => numberFormat(locale, { style: 'unit', unit, unitDisplay: 'long' }).format(n));
  if (words.length === 0)
    words.push(numberFormat(locale, { style: 'unit', unit: 'second', unitDisplay: 'long' }).format(0));
  return cached(`lu|${locale}`, () => new Intl.ListFormat(locale, { type: 'unit', style: 'long' })).format(
    words,
  );
}

/** A calendar date, e.g. "12 Oct 2026" / "Oct 12, 2026" / "2026年10月12日". */
export function formatDate(
  locale: string,
  date: Date | number,
  options: Intl.DateTimeFormatOptions = { dateStyle: 'medium' },
): string {
  return cached(
    `d|${locale}|${JSON.stringify(options)}`,
    () => new Intl.DateTimeFormat(locale, options),
  ).format(date);
}

/** Items joined as a sentence would: "16:9 and 9:16", "a, b und c", "16:9和9:16". */
export function formatList(locale: string, items: string[]): string {
  return cached(`lc|${locale}`, () => new Intl.ListFormat(locale, { type: 'conjunction' })).format(items);
}

/** A relative time such as "5 minutes ago", "yesterday" or "last week" (`numeric: 'auto'`). */
export function formatRelative(locale: string, value: number, unit: Intl.RelativeTimeFormatUnit): string {
  return cached(`r|${locale}`, () => new Intl.RelativeTimeFormat(locale, { numeric: 'auto' })).format(
    value,
    unit,
  );
}
