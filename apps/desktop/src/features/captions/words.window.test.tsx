/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act, fireEvent, screen } from '@testing-library/react';
import { mockIPC } from '@tauri-apps/api/mocks';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { activateLocale } from '../../i18n';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { CaptionWords } from './CaptionWords';
import { forgetCaptionPages } from './useCaptionPages';

const S = 1_000_000;
/** 600 caption pages, one word each, a second apart: a long talk's list. */
const PAGES = Array.from({ length: 600 }, (_, i) => ({
  index: i,
  startUs: i * S,
  endUs: (i + 0.8) * S,
  words: [
    {
      text: `word${i}`,
      joined: false,
      startUs: i * S,
      endUs: (i + 0.8) * S,
      assetId: 'mic',
      sourceStartUs: i * S,
      sourceEndUs: (i + 0.8) * S,
      look: {},
      keyword: null,
    },
  ],
}));

/** The list's scroll box has a layout: 600 px tall. */
const clientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight');
beforeAll(() =>
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains('cap-scroll') ? 600 : 0;
    },
  }),
);
afterAll(() => {
  if (clientHeight) Object.defineProperty(HTMLElement.prototype, 'clientHeight', clientHeight);
  else delete (HTMLElement.prototype as { clientHeight?: number }).clientHeight;
});
/** The list sits at the top of the box's content, so it moves up the screen as the box scrolls. */
const rect = Element.prototype.getBoundingClientRect;
beforeAll(() => {
  Element.prototype.getBoundingClientRect = function (this: Element) {
    const box = this.closest<HTMLElement>('.cap-scroll');
    if (!this.classList.contains('word-lines') || !box) return rect.call(this);
    return { ...rect.call(this), top: -box.scrollTop, y: -box.scrollTop };
  };
});
afterAll(() => {
  Element.prototype.getBoundingClientRect = rect;
});

beforeEach(async () => {
  await activateLocale('en');
  forgetCaptionPages();
  const snap = fixtureSnapshot();
  snap.project.captions = { ...snap.project.captions, enabled: true };
  useProject.setState({ snapshot: snap });
  usePlayback.setState({ playheadUs: 0, playing: false });
  mockIPC((cmd, args: any) => {
    if (cmd !== 'captions_pages') return null;
    const { from, count } = args.query;
    const pages = PAGES.slice(from, from + count);
    return { revision: 0, format: 'portrait', total: 600, first: from, capped: false, pages };
  });
});

const line = (i: number) => document.querySelector(`.word-lines [id$="-line-${i}"]`);
const lines = () => document.querySelectorAll('.word-lines .line').length;
const settle = () => act(() => new Promise((r) => setTimeout(r, 50)));

async function show() {
  renderWithProviders(
    <div className="cap-scroll">
      <CaptionWords format="portrait" />
    </div>,
  );
  await screen.findByRole('listbox', { name: 'Caption words' });
  await settle();
}

describe('a long caption list', () => {
  it('draws the lines near the view, not all 600', async () => {
    await show();
    expect(line(0)).not.toBeNull();
    expect(line(599)).toBeNull();
    expect(lines()).toBeGreaterThan(10);
    expect(lines()).toBeLessThanOrEqual(100);
  });

  it('scrolled half way, draws the lines there instead', async () => {
    await show();
    const box = document.querySelector<HTMLElement>('.cap-scroll')!;
    // Lines are drawn about 52 px tall until measured: line 300 is about 15,600 px down.
    act(() => {
      box.scrollTop = 300 * 52;
      box.dispatchEvent(new Event('scroll'));
    });
    await settle();
    expect(line(300)).not.toBeNull();
    // The line now playing (the first) stays drawn; those between it and the view don't.
    expect(line(100)).toBeNull();
    expect(lines()).toBeLessThanOrEqual(250);
  });

  it('End makes the last word active and draws its line, so the list points at a chip that is there', async () => {
    await show();
    const list = screen.getByRole('listbox', { name: 'Caption words' });
    act(() => list.focus());
    fireEvent.keyDown(list, { key: 'End' });
    await settle();
    expect(line(599)).not.toBeNull();
    const active = list.getAttribute('aria-activedescendant');
    expect(active).toBeTruthy();
    expect(document.getElementById(active!)).toHaveTextContent('word599');
  });

  it('the line now playing is drawn wherever the playhead is', async () => {
    await show();
    act(() => usePlayback.setState({ playheadUs: 450.2 * S }));
    await settle();
    expect(line(450)).toHaveClass('is-now');
  });
});
