/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import { beginCommit } from '../../state/commitQueue';
import { useProject } from '../../state/project';
import { useToasts } from '../../state/toasts';
import { applied, captionsSnapshot as snapshot, resetCaptions, S } from '../../test/captionsBackend';
import { closeAndReopen, closingBackend, heard, renderOpen } from '../../test/closingBackend';
import { renderWithProviders } from '../../test/render';
import { fixField, pagesOf, said, setWordLooks, wordList as list } from '../../test/wordPages';
import { CaptionWords } from './CaptionWords';

/**
 * Closing the project never loses a caption edit still on its way: a fix waiting its turn behind another commit, and
 * the presses kept while the words are being updated, go in before the close and are there when the project opens
 * again; a press that can't go any more is said, never dropped silently.
 */

beforeEach(async () => {
  await resetCaptions();
  useProject.setState({ snapshot: snapshot(true) });
  setWordLooks([]);
  useToasts.setState({ toasts: [] });
});

/** The words "so we then go", with a Rust that saves at close (`closingBackend`). */
function savedProject(wait?: (tx: any) => Promise<void> | undefined) {
  const pages = pagesOf(() => 'so we then go'.split(' ').map((t, i) => [said(t, i, i)]));
  closingBackend({ captions_pages: pages.answer }, wait);
  return pages;
}

async function wordsOpen(open = false) {
  const user = userEvent.setup();
  const view = open
    ? renderOpen(<CaptionWords format="portrait" />)
    : renderWithProviders(<CaptionWords format="portrait" />);
  return { user, box: await list(), unmount: view.unmount };
}

const toasts = () => useToasts.getState().toasts.map((t) => t.text);
const ONE_LOST = 'A change to the words’ look wasn’t saved: the words changed before it could go in.';

describe('closing the project with a caption edit on its way', () => {
  it('a fix saved while another commit is on its way goes in first, and is there once it opens again', async () => {
    savedProject();
    const { user, box, unmount } = await wordsOpen();
    await user.click(within(box).getByRole('option', { name: 'go' }));
    await user.clear(fixField());
    // Another commit is on its way (a slider let go just before), so the fix waits its turn; closing drops what waits.
    const end = beginCommit();
    await user.type(fixField(), 'goes{Enter}');
    expect(applied()).toEqual([]);
    const project = await closeAndReopen({ unmount, landing: end });
    expect(heard()).toEqual(['fixWords', 'close']);
    expect(project.wordFixes).toEqual([{ assetId: 'mic', startUs: 3 * S, endUs: 3.5 * S, text: 'goes' }]);
  });

  it('a field left open as the project closes saves what was typed, for that word', async () => {
    savedProject();
    const { user, box, unmount } = await wordsOpen();
    await user.click(within(box).getByRole('option', { name: 'then' }));
    await user.clear(fixField());
    await user.type(fixField(), 'than');
    const project = await closeAndReopen({ unmount });
    expect(heard()).toEqual(['fixWords', 'close']);
    expect(project.wordFixes).toEqual([{ assetId: 'mic', startUs: 2 * S, endUs: 2.5 * S, text: 'than' }]);
  });

  it('presses kept while the words are being updated go in first, each built on the one before', async () => {
    const pages = savedProject();
    const { user, box, unmount } = await wordsOpen();
    await user.click(within(box).getByRole('option', { name: 'we' }));
    await user.keyboard('{Escape}');
    // The first press lands; its pages are on their way while the next is pressed, and the project closes.
    pages.hold();
    await user.keyboard('h');
    expect(await screen.findByText('Updating…')).toBeInTheDocument();
    await user.keyboard('h');
    expect(applied()).toHaveLength(1);
    const project = await closeAndReopen({ unmount });
    await pages.open();
    expect(heard()).toEqual(['setWordLook', 'setWordLook', 'close']);
    expect(project.wordLooks).toEqual([]);
  });

  it('a press kept that can no longer go is said as it closes, never dropped silently', async () => {
    const pages = savedProject();
    const { user, box, unmount } = await wordsOpen();
    await user.click(within(box).getByRole('option', { name: 'we' }));
    await user.keyboard('{Escape}');
    pages.hold();
    await user.keyboard('h');
    expect(await screen.findByText('Updating…')).toBeInTheDocument();
    await user.keyboard('h');
    // Another word is picked before the words settle: the press was for the word picked before.
    await user.click(within(box).getByRole('option', { name: 'so' }));
    const project = await closeAndReopen({ unmount });
    await pages.open();
    expect(toasts()).toEqual([ONE_LOST]);
    expect(project.wordLooks).toEqual([
      { assetId: 'mic', startUs: 1 * S, endUs: 1.5 * S, look: { hidden: true } },
    ]);
  });

  it('a press kept as the list closes goes in once the commit before it has', async () => {
    const pages = savedProject();
    const { user, box, unmount } = await wordsOpen();
    await user.click(within(box).getByRole('option', { name: 'we' }));
    await user.keyboard('{Escape}');
    pages.hold();
    await user.keyboard('h');
    expect(await screen.findByText('Updating…')).toBeInTheDocument();
    await user.keyboard('h');
    // Back to the Library before the words settle.
    unmount();
    await pages.open();
    await waitFor(() => expect(applied()).toHaveLength(2));
    expect(applied().map((tx: any) => tx.ops[0].look)).toEqual([{ hidden: true }, {}]);
  });

  it('at close, a press kept after this view’s fix landed, its pages still on their way, is said', async () => {
    const pages = savedProject();
    const { user, box, unmount } = await wordsOpen();
    await user.click(within(box).getByRole('option', { name: 'we' }));
    pages.hold();
    await user.clear(fixField());
    await user.type(fixField(), 'wee{Enter}');
    expect(await screen.findByText('Updating…')).toBeInTheDocument();
    // Lit on the word before the fix: a look over it could miss the correction.
    await user.keyboard('h');
    const project = await closeAndReopen({ unmount });
    await pages.open();
    expect(toasts()).toEqual([ONE_LOST]);
    expect(project.wordLooks).toEqual([]);
  });

  it('at close, a press kept while this view’s fix is on its way is said', async () => {
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    savedProject((tx) => (tx.ops[0].type === 'fixWords' ? held : undefined));
    const { user, box, unmount } = await wordsOpen(true);
    await user.click(within(box).getByRole('option', { name: 'we' }));
    await user.clear(fixField());
    await user.type(fixField(), 'wee{Enter}');
    // The fix is on its way: H waits for the word it makes.
    act(() => box.focus());
    await user.keyboard('h');
    const project = await closeAndReopen({ landing: release, unmount });
    expect(toasts()).toEqual([ONE_LOST]);
    expect(heard()).toEqual(['fixWords', 'close']);
    expect(project.wordFixes).toEqual([{ assetId: 'mic', startUs: 1 * S, endUs: 1.5 * S, text: 'wee' }]);
  });
});
