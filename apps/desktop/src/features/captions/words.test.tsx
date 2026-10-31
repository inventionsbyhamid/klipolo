/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { activateLocale } from '../../i18n';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { fixtureSnapshot, revisionPatch } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { fixField } from '../../test/wordPages';
import { CaptionWords } from './CaptionWords';
import { forgetCaptionPages } from './useCaptionPages';

const S = 1_000_000;
const w = (text: string, i: number, extra: Record<string, unknown> = {}) => ({
  text,
  startUs: i * S,
  endUs: (i + 0.8) * S,
  assetId: 'mic',
  sourceStartUs: i * S,
  sourceEndUs: (i + 0.8) * S,
  look: {},
  keyword: null,
  joined: false,
  ...extra,
});
let txs: any[] = [];
let finds: any[] = [];
/** How `captions_find` answers (a test can hold it back). */
let findReply: () => unknown;

const PAGES = [
  {
    index: 0,
    startUs: 0,
    endUs: 3 * S,
    words: [w('THIS', 0, { said: 'This' }), w('IS', 1, { said: 'is' }), w('FREE', 2, { said: 'free' })],
  },
  {
    index: 1,
    startUs: 3 * S,
    endUs: 6 * S,
    words: [
      w('UM,', 3, { said: 'Um,', filler: true }),
      w('TEO', 4, { said: 'Teo', heard: 'Tao' }),
      w('OKAY', 5, { said: 'okay', look: { hidden: true } }),
    ],
  },
  { index: 2, startUs: 6 * S, endUs: 8 * S, words: [w('TEO.', 6, { said: 'Teo.' }), w('BYE', 7, { said: 'bye' })] },
];

beforeEach(async () => {
  await activateLocale('en');
  forgetCaptionPages();
  txs = [];
  finds = [];
  findReply = () => [PAGES[1].words[1], PAGES[2].words[0]];
  const snap = fixtureSnapshot();
  snap.project.captions = { ...snap.project.captions, enabled: true };
  useProject.setState({ snapshot: snap });
  usePlayback.setState({ playheadUs: 0, playing: false });
  mockIPC((cmd, args: any) => {
    if (cmd === 'captions_pages')
      return { revision: 0, format: 'portrait', total: 3, first: 0, capped: false, pages: PAGES };
    if (cmd === 'captions_find') {
      finds.push(args);
      return findReply();
    }
    if (cmd === 'project_apply') {
      txs.push(args.tx);
      return revisionPatch(snap.history.revision + txs.length);
    }
    return null;
  });
});

const list = () => screen.findByRole('listbox', { name: 'Caption words' });

describe('caption words', () => {
  it('hides a word and fixes its spelling, each as one step', async () => {
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    await user.click(within(await list()).getByRole('option', { name: 'BYE' }));
    await user.click(screen.getByRole('button', { name: 'Hide' }));
    await waitFor(() => expect(txs).toHaveLength(1));
    expect(txs[0].label).toBe('captions.hide.undo');
    expect(txs[0].ops).toEqual([
      { type: 'setWordLook', assetId: 'mic', ranges: [{ start: 7 * S, end: 7.8 * S }], look: { hidden: true } },
    ]);
    await user.click(screen.getByRole('option', { name: 'FREE' }));
    // The field starts from what was said, never the style's capitals.
    expect(fixField()).toHaveValue('free');
    await user.clear(fixField());
    await user.type(fixField(), 'free!{Enter}');
    await waitFor(() => expect(txs).toHaveLength(2));
    expect(txs[1].ops[0]).toEqual({ type: 'fixWords', assetId: 'mic', startUs: 2 * S, endUs: 2.8 * S, text: 'free!' });
  });

  it('fixes every place a word shows in one step, or just this one (Fix all)', async () => {
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    await user.click(within(await list()).getByRole('option', { name: 'TEO' }));
    // Asked where else it shows, as said.
    await waitFor(() => expect(finds).toEqual([{ projectId: 'project', format: 'portrait', text: 'Teo' }]));
    await user.clear(fixField());
    await user.type(fixField(), 'Theo');
    const bar = screen.getByRole('group', { name: 'Fix word' });
    expect(bar).toHaveTextContent('Change Teo to Theo. It appears 2 times.');
    expect(within(bar).getByRole('button', { name: 'Just this one' })).toBeInTheDocument();
    // Enter fixes all: each place keeps the marks around it ("Teo." reads "Theo.").
    await user.keyboard('{Enter}');
    await waitFor(() => expect(txs).toHaveLength(1));
    expect(txs[0].label).toBe('captions.fixAll.undo');
    expect(txs[0].params).toEqual({ from: 'Teo', to: 'Theo', count: 2 });
    expect(txs[0].ops).toEqual([
      { type: 'fixWords', assetId: 'mic', startUs: 4 * S, endUs: 4.8 * S, text: 'Theo' },
      { type: 'fixWords', assetId: 'mic', startUs: 6 * S, endUs: 6.8 * S, text: 'Theo.' },
    ]);
    // Just this one: the word alone.
    await user.click(screen.getByRole('option', { name: 'TEO' }));
    await user.clear(fixField());
    await user.type(fixField(), 'Theo');
    await user.click(screen.getByRole('button', { name: 'Just this one' }));
    await waitFor(() => expect(txs).toHaveLength(2));
    expect(txs[1].ops).toEqual([{ type: 'fixWords', assetId: 'mic', startUs: 4 * S, endUs: 4.8 * S, text: 'Theo' }]);
  });

  it('draws fillers dashed, hidden words struck through with a way back, and fixed words with what they were', async () => {
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    expect(within(box).getByRole('option', { name: 'UM,' })).toHaveClass('word', 'is-filler');
    expect(within(box).getByRole('option', { name: 'UM,' })).toHaveAccessibleDescription('Filler');
    const fixed = within(box).getByRole('option', { name: 'TEO' });
    expect(fixed).toHaveClass('is-fixed');
    expect(fixed).toHaveAccessibleDescription('Fixed from Tao');
    const hidden = within(box).getByRole('option', { name: 'OKAY' });
    expect(hidden).toHaveClass('is-hidden');
    expect(hidden).toHaveAccessibleDescription('Hidden. Click to show.');
    // A click shows it again, as one step; it opens no field.
    await user.click(hidden);
    await waitFor(() => expect(txs).toHaveLength(1));
    expect(txs[0].label).toBe('captions.show.undo');
    expect(txs[0].ops).toEqual([
      { type: 'setWordLook', assetId: 'mic', ranges: [{ start: 5 * S, end: 5.8 * S }], look: {} },
    ]);
    expect(screen.queryByRole('textbox', { name: /^Fix the word/ })).toBeNull();
  });

  it('moves the playhead to a word clicked; the keys move, H hides, and Escape lets go', async () => {
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    await user.click(within(box).getByRole('option', { name: 'IS' }));
    expect(usePlayback.getState().playheadUs).toBe(1 * S);
    await user.keyboard('{Escape}');
    act(() => box.focus());
    await user.keyboard('{ArrowRight}');
    expect(within(box).getByRole('option', { selected: true })).toHaveTextContent('FREE');
    expect(box).toHaveAttribute('aria-activedescendant', within(box).getByRole('option', { name: 'FREE' }).id);
    await user.keyboard('h');
    await waitFor(() => expect(txs).toHaveLength(1));
    expect(txs[0].ops[0]).toEqual({
      type: 'setWordLook',
      assetId: 'mic',
      ranges: [{ start: 2 * S, end: 2.8 * S }],
      look: { hidden: true },
    });
    await user.keyboard('{Escape}');
    expect(within(box).queryAllByRole('option', { selected: true })).toHaveLength(0);
  });

  it('says changes stay across styles, without a line of the list for it', async () => {
    renderWithProviders(<CaptionWords format="portrait" />);
    expect(await list()).toHaveAccessibleDescription(
      'Click a word to fix it. Your changes stay when you switch styles.',
    );
    expect(screen.queryByText('Click a word to fix it. Your changes stay when you switch styles.')).toBeNull();
  });

  it('finds a word: only the lines that say it show', async () => {
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const box = await list();
    expect(within(box).getAllByRole('group')).toHaveLength(3);
    await user.type(screen.getByRole('searchbox', { name: 'Find a word' }), 'teo');
    expect(within(box).getAllByRole('group').map((g) => g.getAttribute('aria-label'))).toEqual(['0:03.0', '0:06.0']);
  });

  it('follows the playhead to the line now playing, until the list is scrolled', async () => {
    const scroll = vi.spyOn(Element.prototype, 'scrollIntoView');
    renderWithProviders(
      <div className="cap-scroll">
        <CaptionWords format="portrait" />
      </div>,
    );
    const box = await list();
    const lines = within(box).getAllByRole('group');
    expect(lines[0]).toHaveClass('is-now');
    scroll.mockClear();
    act(() => usePlayback.setState({ playheadUs: 6.5 * S }));
    expect(lines[2]).toHaveClass('is-now');
    expect(lines[0]).not.toHaveClass('is-now');
    expect(scroll.mock.contexts).toContain(lines[2]);
    // The person scrolls the list: it stops following until playback starts again.
    fireEvent.wheel(box.closest('.cap-scroll')!);
    scroll.mockClear();
    act(() => usePlayback.setState({ playheadUs: 3.5 * S }));
    expect(lines[1]).toHaveClass('is-now');
    expect(scroll).not.toHaveBeenCalled();
    act(() => usePlayback.setState({ playing: true }));
    act(() => usePlayback.setState({ playheadUs: 0.5 * S }));
    expect(scroll.mock.contexts).toContain(lines[0]);
    scroll.mockRestore();
  });

  it("brings a word's fix bar into view as it opens, without scrolling to the field first", async () => {
    const user = userEvent.setup();
    renderWithProviders(<CaptionWords format="portrait" />);
    const scroll = vi.spyOn(Element.prototype, 'scrollIntoView');
    const focus = vi.spyOn(HTMLElement.prototype, 'focus');
    await user.click(within(await list()).getByRole('option', { name: 'BYE' }));
    const bar = screen.getByRole('group', { name: 'Fix word' });
    expect(scroll.mock.contexts).toContain(bar);
    const fieldFocus = focus.mock.calls[focus.mock.contexts.indexOf(fixField())];
    expect(fieldFocus).toEqual([{ preventScroll: true }]);
    scroll.mockRestore();
    focus.mockRestore();
  });

});
