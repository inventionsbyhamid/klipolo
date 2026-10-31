import type { CaptionPunctuation } from '../../bindings/CaptionPunctuation';
import edgeKept from '../../lib/caption-edge-kept.json';

/**
 * How the captions space two words: the model's rules, mirrored so "Fix the
 * words" can tell how a fix's edges will be drawn. `caption-join-parity.json`, written by Rust, pins them
 * (`wordFix.parity.test.ts`); what stays at a word's edge is Rust's own table (`caption-edge-kept.json`).
 */

/** The scripts written without spaces between words (Han, kana, their marks and full-width forms; not Hangul). */
const UNSPACED =
  '\\u{2E80}-\\u{303E}\\u{3041}-\\u{312F}\\u{3190}-\\u{31FF}\\u{3300}-\\u{33FF}\\u{3400}-\\u{4DBF}\\u{4E00}-\\u{9FFF}' +
  '\\u{F900}-\\u{FAFF}\\u{FE30}-\\u{FE4F}\\u{FF00}-\\u{FF9F}\\u{20000}-\\u{3FFFD}';
const endsUnspaced = new RegExp(`[${UNSPACED}]$`, 'u');
const startsUnspaced = new RegExp(`^[${UNSPACED}]`, 'u');
const oneUnspaced = new RegExp(`^[${UNSPACED}]$`, 'u');

/** Whether `c` (one character) is of those scripts: the model's `speech::joins_unspaced`. */
export const joinsUnspaced = (c: string) => oneUnspaced.test(c);

/** Whether the captions draw `b` right after `a` without a space: the model's `ends_joining` and `starts_joining`. */
export const joins = (a: string, b: string) => endsUnspaced.test(a) && startsUnspaced.test(b);

/**
 * What may stay at a word's edge in a style that shows only `?` and `!` there (`casing.rs` `edge_kept`): letters,
 * digits and the symbols that belong to a word. Read from the runs Rust wrote over every code point
 * (`caption-edge-kept.json`), so they come from Rust's own Unicode tables whatever Unicode the
 * webview knows, and no copy of the rule can drift. A binary search over about 850 runs.
 */
const KEPT_RUNS = edgeKept.kept as [number, number][];

function keptAtEdge(c: string): boolean {
  const u = c.codePointAt(0) ?? -1;
  let [lo, hi] = [0, KEPT_RUNS.length];
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (KEPT_RUNS[mid][1] < u) lo = mid + 1;
    else hi = mid;
  }
  return lo < KEPT_RUNS.length && KEPT_RUNS[lo][0] <= u;
}

/**
 * The characters of `chars` a style with `punct` shows, as `[from, to)`: all of them, except in Bold, Arcade, Loud and
 * Highlight (`questionAndExclamation`), which trim the marks at a word's edges but keep a minus, a decimal point or an
 * apostrophe right before a digit (`casing.rs` `edges_trimmed`).
 */
function kept(chars: string[], punct: CaptionPunctuation): [number, number] {
  let [from, to] = [0, chars.length];
  if (punct !== 'questionAndExclamation') return [from, to];
  while (from < to && !keptAtEdge(chars[from])) from++;
  while (to > from && !keptAtEdge(chars[to - 1])) to--;
  if (from > 0 && from < to && "-−.'’".includes(chars[from - 1]) && /[0-9]/.test(chars[from])) from--;
  return [from, to];
}

/** `text` as a style with `punct` shows it, its case aside (upper-casing never changes a character's script). */
export function shownEdges(text: string, punct: CaptionPunctuation): string {
  const chars = [...text];
  const [from, to] = kept(chars, punct);
  return chars.slice(from, to).join('');
}

/**
 * Whether the captions draw `b` right after `a`, without a space, in a style with `punct`: judged on what the style
 * shows of the two, as the pages are (`paging.rs`).
 */
export const drawnJoined = (a: string, b: string, punct: CaptionPunctuation) =>
  joins(shownEdges(a, punct), shownEdges(b, punct));

/** Whether the style hides a mark where `a` meets `b`: the last character of `a`, or the first of `b`. */
export function hidesMarkBetween(a: string, b: string, punct: CaptionPunctuation): boolean {
  const [ca, cb] = [[...a], [...b]];
  const [[fromA, toA], [fromB, toB]] = [kept(ca, punct), kept(cb, punct)];
  return (
    (ca.length > 0 && (fromA === toA || toA < ca.length)) || (cb.length > 0 && (fromB === toB || fromB > 0))
  );
}
