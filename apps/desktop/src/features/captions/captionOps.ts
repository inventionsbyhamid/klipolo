import type { CaptionLook } from '../../bindings/CaptionLook';
import type { CaptionPunctuation } from '../../bindings/CaptionPunctuation';
import type { CaptionSettings } from '../../bindings/CaptionSettings';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Op } from '../../bindings/Op';
import type { WordLook } from '../../bindings/WordLook';
import type { WordView } from '../../bindings/WordView';
import { styleOf } from '../../lib/captionStyles';
import type { Label } from '../../lib/labels';
import { useProject } from '../../state/project';
import { commitLatest, commitToggle } from '../../state/commit';

/**
 * The caption edits the panel, the canvas block and the lane make: the same ops chat's tools compile to, each one
 * to, each one undo step through the commit queue, built when it goes in so it builds on the edit before.
 */

const NO_CAPTIONS: CaptionSettings = { enabled: false, place: {}, adSafe: false };

/** The open project's caption settings now. */
export function captionSettings(): CaptionSettings {
  return useProject.getState().snapshot?.project.captions ?? NO_CAPTIONS;
}

/**
 * `setCaptions` with `patch` over the settings as they are now: every other field kept, the style included (never
 * sent as null: a never-enabled project's stays unset, and the model writes the formats' default on the first enable,
 * N3). A field set to undefined is cleared (the style's own look, colour or font).
 */
export function setCaptionsOp(patch: Partial<CaptionSettings>): Op {
  return { type: 'setCaptions', captions: { ...captionSettings(), ...patch } };
}

/** A position word's centre per format: the model's `place_y`, inside the organic-safe band in 9:16. */
export type PlaceWord = 'top' | 'middle' | 'bottom';
export const PLACE_Y: Record<PlaceWord, Record<FormatKind, number>> = {
  top: { portrait: 0.22, square: 0.18, landscape: 0.14 },
  middle: { portrait: 0.5, square: 0.5, landscape: 0.5 },
  bottom: { portrait: 0.7, square: 0.85, landscape: 0.88 },
};

/** The word `y` is in `format`, or null for the style's own place or one dragged elsewhere. */
export function placeWord(y: number | undefined, format: FormatKind): PlaceWord | null {
  if (y === undefined) return null;
  const words = Object.keys(PLACE_Y) as PlaceWord[];
  return words.find((w) => Math.abs(PLACE_Y[w][format] - y) < 1e-6) ?? null;
}

/** The block's centre in `format` at `y` (a share of the frame's height), or the style's own again (null). */
export function placeOp(format: FormatKind, y: number | null): Op {
  const place = { ...captionSettings().place };
  delete place[format];
  if (y !== null) place[format] = Math.round(y * 1e4) / 1e4;
  return setCaptionsOp({ place });
}

/**
 * A fix of `words` (a run of a page's, in order, of one file: `fixOps` stops at another file's) to `text`: from the
 * first word's start to the last's end in their file's own time. Only the first word's file. Null for no words.
 */
export function fixOp(words: WordView[], text: string): Op | null {
  const first = words[0];
  if (!first) return null;
  const own = words.filter((w) => w.assetId === first.assetId);
  return {
    type: 'fixWords',
    assetId: first.assetId,
    startUs: first.sourceStartUs,
    endUs: own[own.length - 1].sourceEndUs,
    text,
  };
}

/** How the project's caption style shows the marks at a word's edges now: a fix is judged in it. */
export const captionPunctuation = (): CaptionPunctuation =>
  styleOf(captionSettings(), useProject.getState().snapshot?.project.settings.formats ?? []).punctuation;

/** A change of the word list's look bar: parts of a look (undefined: the style's), a size step, or none. */
export type LookChange = Partial<CaptionLook> | 'reset' | 'bigger' | 'smaller';

/**
 * Whether `w` is a keyword: the person's "always" or "never" first, else the style's pick (`keyword`).
 * On the pages that is `keyword !== null || look.emphasis === true`: in a style with keywords the pages already force
 * "always" and drop "never", and in Boxed, Clean, Highlight and Karaoke, which have none, the engine draws `emphasis`
 * itself. The person's word first keeps it right on a look just set, before its pages come back (the look bar's).
 */
export const isKeyword = (w: WordView): boolean => w.look.emphasis ?? w.keyword !== null;

/** The Keyword toggle's change for `words`: never a keyword when every word is one, else always one. */
export const keywordChange = (words: WordView[]): LookChange => ({ emphasis: !words.every(isKeyword) });

/** `look` without its unset parts: what Rust stores, so two looks compare by their parts. */
function tidyLook(look: CaptionLook): CaptionLook {
  return Object.fromEntries(Object.entries(look).filter(([, v]) => v !== undefined)) as CaptionLook;
}

/** A look's parts in one order, to group words by the look they take. */
const lookKey = (look: CaptionLook) =>
  JSON.stringify(Object.entries(look).sort(([a], [b]) => a.localeCompare(b)));

/**
 * `look` with `change` over it, as chat's `styleCaptionWords` merges (`caption_looks.rs`): the parts it names, the
 * rest kept; a size steps from the word's own, within −2 to 3 (the style's size is no step); 'reset' is the style's.
 */
export function withChange(look: CaptionLook, change: LookChange): CaptionLook {
  if (change === 'reset') return {};
  if (change === 'bigger' || change === 'smaller') {
    const now = look.size ?? 0;
    const next = change === 'bigger' ? Math.min(now + 1, 3) : Math.max(now - 1, -2);
    return tidyLook({ ...look, size: next === 0 ? undefined : next });
  }
  return tidyLook({ ...look, ...change });
}

/**
 * The ops that give `words` (each once) `change` over their own looks: a word the change leaves alike
 * sends nothing, and the words that come out with one look are one `setWordLook` over their ranges in their file's own
 * in one pass. One undo step whatever the number of words.
 */
export function lookOps(words: WordView[], change: LookChange): Op[] {
  const ops: Extract<Op, { type: 'setWordLook' }>[] = [];
  const byLook = new Map<string, number>();
  const seen = new Set<string>();
  for (const w of words) {
    const word = `${w.assetId}@${w.sourceStartUs}`;
    if (seen.has(word)) continue;
    seen.add(word);
    const look = withChange(w.look, change);
    if (lookKey(look) === lookKey(tidyLook(w.look))) continue;
    const range = { start: w.sourceStartUs, end: w.sourceEndUs };
    const group = `${w.assetId}@${lookKey(look)}`;
    const at = byLook.get(group);
    if (at === undefined) {
      byLook.set(group, ops.length);
      ops.push({ type: 'setWordLook', assetId: w.assetId, ranges: [range], look });
    } else ops[at].ranges.push(range);
  }
  return ops;
}

/**
 * The person's look for `w` as the project holds it now (by its middle in its file's own time, as the pages find it),
 * so changes made in a row build on each other before the pages are asked again; the page's own while the project
 * keeps no looks list yet (none was ever set).
 */
export function currentLook(
  w: WordView,
  looks: WordLook[] | undefined = useProject.getState().snapshot?.project.wordLooks,
): CaptionLook {
  if (!looks) return w.look;
  // Where the pages read it: the middle of the piece a correction shows, else of its range.
  const mid = w.lookUs ?? w.sourceStartUs + Math.floor((w.sourceEndUs - w.sourceStartUs) / 2);
  // Sorted by file and start, never overlapping: the last one starting at or before `mid`.
  let [lo, hi] = [0, looks.length];
  while (lo < hi) {
    const m = (lo + hi) >> 1;
    const l = looks[m];
    if (l.assetId < w.assetId || (l.assetId === w.assetId && l.startUs <= mid)) lo = m + 1;
    else hi = m;
  }
  const l = looks[lo - 1];
  return l && l.assetId === w.assetId && mid < l.endUs ? l.look : {};
}

/**
 * One caption settings change as an undo step, built from the settings as they are when it goes in. Closing the project
 * waits for one still waiting its turn (`keep`): each carries the whole settings, so they go
 * in order, each on the one before.
 */
export function commitCaptions(label: Label, patch: () => Partial<CaptionSettings>): Promise<boolean> {
  return commitLatest(label, () => [setCaptionsOp(patch())], { keep: true });
}

/** The block's centre in `format` at `y`, or the style's own place again (null), as one "Move captions" step. */
export function commitPlace(format: FormatKind, y: number | null): Promise<boolean> {
  return commitLatest('captions.place.undo', () => [placeOp(format, y)], { keep: true });
}

/**
 * Captions on or off as one undo step decided as it goes in, so two quick presses turn them on and back off. Closing
 * the project waits for it, in its turn.
 */
export async function toggleCaptions(): Promise<void> {
  await commitToggle(
    () => captionSettings().enabled,
    (enabled) => (enabled ? 'captions.on.undo' : 'captions.off.undo'),
    (enabled) => [setCaptionsOp({ enabled })],
    { keep: true },
  );
}
