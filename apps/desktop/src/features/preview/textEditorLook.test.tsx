import { act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeAll, beforeEach, describe, expect, it } from 'vitest';
import type { TextAlign } from '../../bindings/TextAlign';
import { activateLocale } from '../../i18n';
import { previewScale } from '../../lib/preview';
import { caretColor } from '../../lib/textLook';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { useUi } from '../../state/ui';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { editor, heard, rustForText, titleBox } from '../../test/textEdit';
import { Stage } from './Stage';
import { wrapWidth } from './textBox';
import { MAX_TEXT, useTextEdit } from './textEdit';

beforeAll(() => {
  // jsdom has no layout: give the stage an 800×450 box (a 16:9 canvas of 800×450).
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 450 });
});

beforeEach(async () => {
  await activateLocale('en');
  heard.reset();
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false, seq: 0 });
  useUi.setState({ previewFormat: null });
  useTextEdit.setState({ session: null, closing: null });
  rustForText();
});

/** The fixture project with its title aligned `align`, half transparent. */
function withTitle(align: TextAlign) {
  const snap = fixtureSnapshot();
  const items = snap.project.timeline.tracks[0].items;
  const content = items[0].content as Extract<(typeof items)[0]['content'], { type: 'text' }>;
  items[0] = {
    ...items[0],
    layout: { ...items[0].layout, opacity: 0.5 },
    content: { ...content, style: { ...content.style, align } },
  };
  useProject.setState({ snapshot: snap, pending: 0 });
}

describe('the canvas editor’s look', () => {
  it.each([
    ['left', 'flex-start'],
    ['center', 'center'],
    ['right', 'flex-end'],
  ] as const)(
    'a %s-aligned title keeps its aligned edge where the engine draws it, wrapping at the engine’s width',
    async (align, justify) => {
      withTitle(align);
      renderWithProviders(<Stage />);
      await userEvent.dblClick(titleBox());
      const cell = editor().parentElement!;
      const box = cell.parentElement!;
      // The engine's slack goes after a left title's box, before a right one's, either side of a centred one.
      expect(box.style.justifyContent).toBe(justify);
      // The box is 0.6 of 800 px: the engine wraps at 480 × 1.05 + 4, never wider.
      expect(cell.style.width).toBe('508px');
      expect(cell.style.gridTemplateColumns).toBe('minmax(0, 1fr)');
      // Typed at full opacity, whatever the layer's own.
      expect(cell.style.opacity).toBe('1');
    },
  );

  it('a full-width title in the 9:16 view wraps at the engine’s width: its box plus the slack', async () => {
    // A landscape title 0.6 of 1920 px is wider than a 9:16 frame: its box there is the whole width (w = 1).
    withTitle('center');
    const snap = useProject.getState().snapshot!;
    useProject.setState({
      snapshot: {
        ...snap,
        project: {
          ...snap.project,
          settings: { ...snap.project.settings, formats: ['landscape', 'portrait'] },
        },
      },
    });
    useUi.setState({ previewFormat: 'portrait' });
    renderWithProviders(<Stage />);
    await userEvent.dblClick(titleBox());
    const cell = editor().parentElement!;
    // The 9:16 canvas in an 800×450 area is 253 px wide; the engine judges its slack against the text's own texture
    // (box plus padding), so it always wraps at box × 1.05 + 4, never at the frame's width.
    expect(parseFloat(cell.style.width)).toBeCloseTo(253 * 1.05 + 4);
    expect(wrapWidth(1080)).toBeCloseTo(1138);
  });

  it('the slack’s 4 px are the preview’s own pixels: on a 2× screen within the pixel budget, 2 CSS px', () => {
    // 800×450 CSS at 2× is 1.44 Mpx, over a 0.92 Mpx budget: the preview draws 1280×720, 1.6 of its pixels per CSS px.
    const unit = previewScale({ width: 800, height: 450 }, 2, 921_600);
    expect(unit).toBeCloseTo(1.6);
    expect(wrapWidth(480, unit)).toBeCloseTo(480 * 1.05 + 4 / 1.6);
    expect(wrapWidth(480, previewScale({ width: 800, height: 450 }, 2, 10_000_000))).toBeCloseTo(506);
  });

  it('the caret shows beside the letters: the outline’s colour, else dark by light letters and light by dark', () => {
    const outline = { color: '#FF0000', width: 4 };
    expect(caretColor({ color: '#FFFFFF', outline: null })).toBe('#000000');
    expect(caretColor({ color: '#FFE135', outline: null })).toBe('#000000');
    expect(caretColor({ color: '#000000', outline: null })).toBe('#FFFFFF');
    expect(caretColor({ color: '#1E40AF', outline: null })).toBe('#FFFFFF');
    expect(caretColor({ color: '#FFFFFF', outline })).toBe('#FF0000');
  });

  it('the length cap never splits an emoji', async () => {
    withTitle('center');
    renderWithProviders(<Stage />);
    await userEvent.dblClick(titleBox());
    act(() => useTextEdit.getState().setText(`${'a'.repeat(MAX_TEXT - 1)}😀`));
    const words = useTextEdit.getState().session!.text;
    expect(words).toBe('a'.repeat(MAX_TEXT - 1));
    act(() => useTextEdit.getState().setText(`${'a'.repeat(MAX_TEXT - 2)}😀`));
    expect(useTextEdit.getState().session!.text).toBe(`${'a'.repeat(MAX_TEXT - 2)}😀`);
  });
});
