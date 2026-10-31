import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { Item } from '../../bindings/Item';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { rustForText } from '../../test/textEdit';
import { Stage } from './Stage';
import { useTextEdit } from './textEdit';

/**
 * What a press on the canvas reaches: the selected layer, and every layer drawn after it, are lifted; layers keep their
 * draw order among themselves, so a title over a selected image is still on top of it; and a title being typed is
 * above them all. jsdom has no hit testing, so "above" is the stacking order of the gizmo layer's children: z-index,
 * then document order.
 */
function above(a: HTMLElement, b: HTMLElement): boolean {
  const z = (el: HTMLElement) => Number(el.style.zIndex || 0);
  if (z(a) !== z(b)) return z(a) > z(b);
  return (b.compareDocumentPosition(a) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
}

/** An image under the title (the same track, drawn first), over the same part of the frame. */
const image: Item = {
  ...titleItem,
  id: 'logo-layer',
  layout: { ...titleItem.layout, rect: { x: 0.1, y: 0.05, w: 0.8, h: 0.3 } },
  content: { type: 'image', assetId: 'logo' },
};

beforeEach(() => {
  rustForText();
  const snap = fixtureSnapshot();
  const t1 = snap.project.timeline.tracks[0];
  snap.project.timeline.tracks[0] = { ...t1, items: [image, ...t1.items] };
  useProject.setState({ snapshot: snap, pending: 0 });
  // The title and the image show from 1 s to 4 s.
  usePlayback.setState({ playheadUs: 1_500_000 });
  // jsdom has no layout: the stage gets a 450×800 box.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 450 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 800 });
});
afterEach(() => {
  useTextEdit.setState({ session: null, closing: null });
  delete (HTMLElement.prototype as { clientWidth?: number }).clientWidth;
  delete (HTMLElement.prototype as { clientHeight?: number }).clientHeight;
});

function stage() {
  renderWithProviders(<Stage />);
}

describe('what a press on the canvas reaches', () => {
  it('a title over the selected image can still be picked and typed into', async () => {
    useSelection.setState({ selectedId: 'logo-layer' });
    stage();
    const picture = screen.getByRole('button', { name: /^Image: logo/ });
    const title = screen.getByRole('button', { name: /^Select Set up billing/ });
    expect(picture.style.zIndex).toBe('1');
    expect(above(title, picture)).toBe(true);
    await userEvent.click(title);
    expect(useSelection.getState().selectedId).toBe('title');
    await userEvent.dblClick(screen.getByRole('button', { name: /^Text: Set up billing/ }));
    expect(useTextEdit.getState().session?.id).toBe('title');
  });

  it('a new title typed over the selected layer is above it', async () => {
    useSelection.setState({ selectedId: 'title' });
    stage();
    const selected = screen.getByRole('button', { name: /^Text: Set up billing/ });
    useTextEdit.getState().beginDraft({ ...titleItem, id: 'draft' }, false);
    const editor = await screen.findByRole('textbox');
    // The editor's own box among the gizmo layer's children.
    let box: HTMLElement = editor;
    while (box.parentElement !== selected.parentElement) box = box.parentElement!;
    expect(above(box, selected)).toBe(true);
  });
});
