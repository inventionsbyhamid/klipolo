import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import { useEditorShortcuts } from '../command-palette/useEditorShortcuts';
import { useProject } from '../../state/project';
import { revisionPatch } from '../../test/fixtures';
import {
  applied,
  callsOf,
  captionsBackend as backend,
  captionsSnapshot as snapshot,
  resetCaptions,
} from '../../test/captionsBackend';
import { renderWithProviders } from '../../test/render';
import { fixField, lit, nextRevision, pagesOf, said, setWordLooks, wordList as list } from '../../test/wordPages';
import { CaptionWords } from './CaptionWords';

/** The word list while its pages are on their way, and after edits. */

beforeEach(async () => {
  await resetCaptions();
  useProject.setState({ snapshot: snapshot(true) });
  setWordLooks([]);
});

describe('the word list during a round trip and after edits', () => {
  it('waits while an edit’s pages are on their way, so a fix never covers a word that moved (round trip)', async () => {
    const words = 'so we then go on now'.split(' ');
    let order = [0, 1, 2, 3, 4, 5];
    const pages = pagesOf(() =>
      order.map((src, i) => [said(words[src], i, src, src + 0.5, i > 0 && src !== order[i - 1] + 1 ? { cutBefore: true } : {})]),
    );
    backend({ captions_pages: pages.answer, project_apply: () => revisionPatch(4) });
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    await user.click(within(box).getByRole('option', { name: 'then' }));
    await user.clear(fixField());
    await user.type(fixField(), 'than');
    // Chat moves "then go" to the end; its pages are on their way when Enter is pressed.
    pages.hold();
    order = [0, 1, 4, 5, 2, 3];
    nextRevision();
    expect(await screen.findByText('Updating…')).toBeInTheDocument();
    await user.keyboard('{Enter}');
    expect(applied()).toEqual([]);
    await pages.open();
    // Found again where it plays now, and fixed there: one range of its own file.
    await waitFor(() => expect(applied()).toHaveLength(1));
    expect(applied()[0].ops).toEqual([
      { type: 'fixWords', assetId: 'mic', startUs: 2_000_000, endUs: 2_500_000, text: 'than' },
    ]);
  });

  it('follows its word when an edit before it moves it on the timeline', async () => {
    let shift = 0;
    const pages = pagesOf(() => 'so we then go'.split(' ').map((t, i) => [said(t, i + 1 - shift, i)]));
    backend({ captions_pages: pages.answer });
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    await user.click(within(box).getByRole('option', { name: 'we' }));
    await user.keyboard('{Escape}');
    // Chat removed a pause before them: every word plays 0.4 s earlier.
    shift = 0.4;
    nextRevision();
    // The new pages are in (the first now starts at 0:00.6), and the selection lights the same word there.
    await within(box).findByRole('group', { name: '0:00.6' });
    await waitFor(() => expect(lit(box)).toEqual(['we']));
  });

  it('keeps what was typed through an edit before the word', async () => {
    let shift = 0;
    const pages = pagesOf(() => ['so', 'we', 'go'].map((t, i) => [said(t, i + 1 - shift, i)]));
    backend({ captions_pages: pages.answer });
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    await user.click(within(box).getByRole('option', { name: 'we' }));
    await user.clear(fixField());
    await user.type(fixField(), 'wee');
    shift = 0.4;
    nextRevision();
    await within(box).findByRole('group', { name: '0:00.6' });
    expect(fixField()).toHaveValue('wee');
  });

  it('comes back to the word picked when a correction it followed into is undone', async () => {
    let fixed = false;
    const pages = pagesOf(() =>
      fixed
        ? [[said('so', 0, 0)], [said('we’n', 1, 1, 2.5)], [said('go', 3, 3)]]
        : [[said('so', 0, 0)], [said('we', 1, 1)], [said('then', 2, 2)], [said('go', 3, 3)]],
    );
    backend({ captions_pages: pages.answer });
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    await user.click(within(box).getByRole('option', { name: 'then' }));
    await user.keyboard('{Escape}');
    // Chat fixes "we then" to "we’n": the selection follows into the correction.
    fixed = true;
    nextRevision();
    await waitFor(() => expect(lit(box)).toEqual(['we’n']));
    // Undone (Command Z): the correction is gone, and the selection is "then" again.
    fixed = false;
    nextRevision();
    await waitFor(() => expect(lit(box)).toEqual(['then']));
  });

  it('comes back to the word picked when the middle of the undone correction is another word', async () => {
    let fixed = false;
    const pages = pagesOf(() =>
      fixed
        ? [[said('so', 0, 0)], [said('wego', 1, 1, 3.5)], [said('now', 4, 4)]]
        : 'so we then go now'.split(' ').map((t, i) => [said(t, i, i)]),
    );
    backend({ captions_pages: pages.answer });
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    await user.click(within(box).getByRole('option', { name: 'go' }));
    await user.keyboard('{Escape}');
    fixed = true;
    nextRevision();
    await waitFor(() => expect(lit(box)).toEqual(['wego']));
    // The correction's middle (2.25 s) is "then" again: the selection is still the word picked, "go".
    fixed = false;
    nextRevision();
    await waitFor(() => expect(lit(box)).toEqual(['go']));
  });

  it('undoes with Command Z while the word list has the focus', async () => {
    const pages = pagesOf(() => 'so we then go'.split(' ').map((t, i) => [said(t, i, i)]));
    backend({ captions_pages: pages.answer, project_undo: () => revisionPatch(4) });
    function Harness() {
      useEditorShortcuts();
      return <CaptionWords format="portrait" />;
    }
    renderWithProviders(<Harness />);
    const box = await list();
    await userEvent.click(within(box).getByRole('option', { name: 'so' }));
    await userEvent.keyboard('{Escape}');
    expect(box).toHaveFocus();
    await userEvent.keyboard('{Meta>}z{/Meta}');
    await waitFor(() => expect(callsOf('project_undo')).toHaveLength(1));
    // The list keeps its own keys: an arrow moves the word, never steps the playhead.
    await userEvent.keyboard('{ArrowRight}');
    expect(lit(box)).toEqual(['we']);
  });
});
