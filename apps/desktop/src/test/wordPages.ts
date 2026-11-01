/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, screen, within } from '@testing-library/react';
import type { WordLook } from '../bindings/WordLook';
import { useProject } from '../state/project';
import { pagesAnswer, S, word } from './captionsBackend';
import { revisionPatch } from './fixtures';

/**
 * The caption word list's mocked project for the tests of presses made while its words are being updated: pages of
 * words with the looks applied to them, answered as `captions_pages` answers and held on demand.
 */

export type W = ReturnType<typeof word>;

/** A word heard at `at` s from second `src` of the mic (half a second long, unless `to`). */
export const said = (
  text: string,
  at: number,
  src: number,
  to = src + 0.5,
  extra: Record<string, unknown> = {},
) => word(text, at, { endUs: (at + 0.5) * S, sourceStartUs: src * S, sourceEndUs: to * S, ...extra });

/** The mocked project's looks, kept as Rust keeps them for whole words, and read where the pages read them. */
let looks: WordLook[] = [];
export const wordLooks = () => looks;
export const setWordLooks = (next: WordLook[]) => (looks = next);
const midOf = (w: W) => (w as { lookUs?: number }).lookUs ?? (w.sourceStartUs + w.sourceEndUs) / 2;
const lookOf = (w: W) =>
  looks.find((l) => l.assetId === w.assetId && l.startUs <= midOf(w) && midOf(w) < l.endUs)?.look ?? {};

/** The next revision with the looks now (`project_apply`, `project_undo`). */
export const patched = () => {
  const p = revisionPatch(useProject.getState().snapshot!.history.revision + 1);
  return { ...p, patch: { ...p.patch, wordLooks: [...looks] } };
};

/** `project_apply`: applies a transaction's looks; `onFix` hears its fixes. */
export const applying =
  (onFix: () => void = () => {}) =>
  ({ tx }: { tx: { ops: any[] } }) => {
    for (const op of tx.ops) {
      if (op.type === 'fixWords') onFix();
      if (op.type !== 'setWordLook') continue;
      for (const r of op.ranges) {
        looks = looks.filter((l) => l.assetId !== op.assetId || l.endUs <= r.start || l.startUs >= r.end);
        if (Object.keys(op.look).length > 0)
          looks.push({ assetId: op.assetId, startUs: r.start, endUs: r.end, look: op.look });
      }
    }
    looks.sort((a, b) => a.startUs - b.startUs);
    return patched();
  };

type Query = { type: string; from?: number; count?: number };
/** The project's pages now, with their looks, as `captions_pages` answers; held while `gate` is shut (`which` ones). */
export function pagesOf(groups: () => W[][]) {
  let gate: Promise<void> | null = null;
  let which: (q: Query) => boolean = () => true;
  let open = () => {};
  const answer = async ({ query }: { query: Query }) => {
    const all = groups().map((words, index) => ({
      index,
      startUs: words[0].startUs,
      endUs: words[words.length - 1].endUs,
      words: words.map((w) => ({ ...w, look: lookOf(w) })),
    }));
    const from = query.from ?? 0;
    const held = gate;
    if (held && which(query)) await held;
    return { ...pagesAnswer(all.slice(from, from + (query.count ?? 0))), total: all.length, first: from };
  };
  return {
    answer,
    hold: (only: (q: Query) => boolean = () => true) => {
      which = only;
      gate = new Promise((r) => (open = () => ((gate = null), r())));
    },
    open: () => act(async () => open()),
  };
}

/** Another edit landed (chat's): the pages are asked again. */
export const nextRevision = () =>
  act(() =>
    useProject.setState((s) => ({
      snapshot: {
        ...s.snapshot!,
        history: { ...s.snapshot!.history, revision: s.snapshot!.history.revision + 1 },
      },
    })),
  );

/** The words picked in the list. */
export const lit = (box: HTMLElement) =>
  within(box)
    .queryAllByRole('option', { selected: true })
    .map((o) => o.textContent);
export const wordList = () => screen.findByRole('listbox', { name: 'Caption words' });

/** The field a word is fixed in. */
export const fixField = () => screen.getByRole('textbox', { name: /^Fix the word / });
