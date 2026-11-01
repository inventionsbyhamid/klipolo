/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import type { WordLook } from '../../bindings/WordLook';
import { useProject } from '../../state/project';
import { useAnnouncer } from '../timeline/announce';
import {
  applied,
  captionsBackend as backend,
  captionsSnapshot as snapshot,
  landed,
  resetCaptions,
  S,
} from '../../test/captionsBackend';
import { renderWithProviders } from '../../test/render';
import {
  applying,
  fixField,
  lit,
  nextRevision,
  pagesOf,
  patched,
  said,
  setWordLooks,
  wordList as list,
  wordLooks,
} from '../../test/wordPages';
import { CaptionWords } from './CaptionWords';

/**
 * Hides and fixes made while "Updating…" shows: kept, and sent once the words settle when nothing else changed, or
 * dropped and said; never lost silently.
 */

beforeEach(async () => {
  await resetCaptions();
  useProject.setState({ snapshot: snapshot(true) });
  setWordLooks([]);
});

const looksSent = () =>
  applied()
    .filter((tx: any) => tx.ops[0].type === 'setWordLook')
    .map((tx: any) => tx.ops);
const PRESS_AGAIN = 'The words changed. Press again.';

describe('presses made while the words are being updated', () => {
  it('keeps them and sends each once the words settle: H twice is two steps', async () => {
    const pages = pagesOf(() => 'so we then go'.split(' ').map((t, i) => [said(t, i, i)]));
    backend({ captions_pages: pages.answer, project_apply: applying() });
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    await user.click(within(box).getByRole('option', { name: 'we' }));
    await user.keyboard('{Escape}');
    // The first press lands; its pages are on their way while the next is pressed.
    pages.hold();
    await user.keyboard('h');
    expect(await screen.findByText('Updating…')).toBeInTheDocument();
    await user.keyboard('h');
    // Nothing more goes while it waits…
    expect(looksSent()).toHaveLength(1);
    await pages.open();
    // …and then the press kept goes, built on the one before: shown again.
    await waitFor(() => expect(looksSent()).toHaveLength(2));
    expect(looksSent().map((ops: any) => ops[0].look)).toEqual([{ hidden: true }, {}]);
    expect(looksSent().every((ops: any) => ops[0].ranges[0].start === 1 * S)).toBe(true);
    expect(screen.queryByText(PRESS_AGAIN)).toBeNull();
  });

  it('hides the correction when Hide is pressed while the fix is on its way', async () => {
    let fixed = false;
    const pages = pagesOf(() =>
      'so we then go'
        .split(' ')
        .map((t, i) => [said(fixed && i === 1 ? 'wee' : t, i, i, i + 0.5, fixed && i === 1 ? { heard: 'we' } : {})]),
    );
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    const apply = applying(() => (fixed = true));
    backend({
      captions_pages: pages.answer,
      project_apply: async (args: any) => {
        if (args.tx.ops[0].type === 'fixWords') await held;
        return apply(args);
      },
    });
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    await user.click(within(box).getByRole('option', { name: 'we' }));
    await user.clear(fixField());
    await user.type(fixField(), 'wee{Enter}');
    // The fix is on its way; H on the word waits for the word it makes.
    act(() => box.focus());
    await user.keyboard('h');
    expect(applied()).toHaveLength(1);
    expect(applied()[0].ops[0].type).toBe('fixWords');
    await act(async () => release());
    await waitFor(() => expect(looksSent()).toHaveLength(1));
    expect(looksSent()[0]).toEqual([
      { type: 'setWordLook', assetId: 'mic', ranges: [{ start: 1 * S, end: 1.5 * S }], look: { hidden: true } },
    ]);
    await waitFor(() => expect(lit(box)).toEqual(['wee']));
  });

  it('drops a press when Command Z comes before the words settle, and says so', async () => {
    const pages = pagesOf(() => 'so we then go'.split(' ').map((t, i) => [said(t, i, i)]));
    let before: WordLook[] = [];
    const apply = applying();
    backend({
      captions_pages: pages.answer,
      project_apply: (args: any) => {
        before = [...wordLooks()];
        return apply(args);
      },
      project_undo: () => {
        setWordLooks(before);
        return patched();
      },
    });
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    await user.click(within(await list()).getByRole('option', { name: 'we' }));
    await user.keyboard('{Escape}');
    pages.hold();
    await user.keyboard('h');
    expect(await screen.findByText('Updating…')).toBeInTheDocument();
    await user.keyboard('h');
    await act(async () => {
      await useProject.getState().undo();
    });
    await pages.open();
    expect(await screen.findByText(PRESS_AGAIN)).toBeInTheDocument();
    // Said for VoiceOver too, through the editor's live region.
    expect(useAnnouncer.getState().message).toBe(PRESS_AGAIN);
    expect(looksSent()).toHaveLength(1);
  });

  it('drops a press when chat’s correction takes in a word that wasn’t lit, and says so', async () => {
    let fixed = false;
    const pages = pagesOf(() =>
      fixed
        ? [[said('so', 0, 0)], [said('we’n', 1, 1, 2.5, { lookUs: 1.75 * S })], [said('go', 3, 3)]]
        : 'so we then go'.split(' ').map((t, i) => [said(t, i, i)]),
    );
    backend({ captions_pages: pages.answer, project_apply: applying() });
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    await user.click(within(box).getByRole('option', { name: 'we' }));
    await user.keyboard('{Escape}');
    pages.hold();
    await user.keyboard('h');
    expect(await screen.findByText('Updating…')).toBeInTheDocument();
    await user.keyboard('h');
    // Chat fixes "we then" as one word meanwhile: the selection follows "we" into it, "then" with it.
    fixed = true;
    nextRevision();
    await pages.open();
    await waitFor(() => expect(lit(box)).toEqual(['we’n']));
    expect(await screen.findByText(PRESS_AGAIN)).toBeInTheDocument();
    expect(looksSent()).toHaveLength(1);
  });
});

describe('a spelling left while the words are being updated', () => {
  it('is saved once they settle, though another word was picked meanwhile', async () => {
    let shift = 0;
    const pages = pagesOf(() => 'so we then go now'.split(' ').map((t, i) => [said(t, i + 1 - shift, i)]));
    backend({ captions_pages: pages.answer, project_apply: applying() });
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    await user.click(within(box).getByRole('option', { name: 'go' }));
    await user.clear(fixField());
    await user.type(fixField(), 'goes');
    // Chat removes a pause before them; while its pages are on their way, another word is clicked.
    pages.hold();
    shift = 0.4;
    nextRevision();
    expect(await screen.findByText('Updating…')).toBeInTheDocument();
    await user.click(within(box).getByRole('option', { name: 'now' }));
    expect(applied()).toEqual([]);
    await pages.open();
    expect((await landed()).at(-1).ops).toEqual([
      { type: 'fixWords', assetId: 'mic', startUs: 3 * S, endUs: 3.5 * S, text: 'goes' },
    ]);
    // The word clicked is the one open for fixing, through the edits.
    await waitFor(() => expect(fixField()).toHaveAccessibleName('Fix the word now'));
  });

  it('says it wasn’t saved when its word changed meanwhile', async () => {
    let last = 'go';
    const pages = pagesOf(() => ['so', 'we', 'then', last].map((t, i) => [said(t, i, i)]));
    backend({ captions_pages: pages.answer, project_apply: applying() });
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    await user.click(within(box).getByRole('option', { name: 'go' }));
    await user.clear(fixField());
    await user.type(fixField(), 'goes');
    // Chat rewords the same word meanwhile: the spelling waits, then isn't saved.
    pages.hold();
    last = 'gone';
    nextRevision();
    expect(await screen.findByText('Updating…')).toBeInTheDocument();
    await user.keyboard('{Enter}');
    await pages.open();
    const unsaved = 'Your spelling “goes” wasn’t saved: the words changed while you typed.';
    expect(await screen.findByText(unsaved)).toBeInTheDocument();
    // Said for VoiceOver too.
    expect(useAnnouncer.getState().message).toBe(unsaved);
    expect(applied()).toEqual([]);
    // Typed again on the word as it reads now, it goes.
    await user.click(within(box).getByRole('option', { name: 'gone' }));
    await user.clear(fixField());
    await user.type(fixField(), 'goes{Enter}');
    await waitFor(() =>
      expect(applied().at(-1)?.ops).toEqual([
        { type: 'fixWords', assetId: 'mic', startUs: 3 * S, endUs: 3.5 * S, text: 'goes' },
      ]),
    );
  });
});
