import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { Library } from '../../layout/Library';
import { useProject } from '../../state/project';
import {
  applied,
  calls,
  callsOf,
  captionsBackend as backend,
  captionsSnapshot as snapshot,
  landed,
  resetCaptions,
} from '../../test/captionsBackend';
import { renderWithProviders } from '../../test/render';
import { StyleCards } from './StyleCards';

beforeEach(resetCaptions);

const cards = () => within(screen.getByRole('radiogroup', { name: 'Caption style' })).getAllByRole('radio');

describe('caption style cards', () => {
  it('shows Off and the five styles, each an existing builtin, in the design’s order', () => {
    backend();
    renderWithProviders(<StyleCards />);
    expect(cards().map((c) => c.getAttribute('aria-label'))).toEqual([
      'Off',
      'Clean',
      'Bold pop',
      'Karaoke',
      'Highlight',
      'Boxed',
    ]);
    // Off is chosen while captions are off.
    expect(screen.getByRole('radio', { name: 'Off' })).toHaveAttribute('aria-checked', 'true');
  });

  it('turns captions on as one undo step, without choosing a style itself', async () => {
    backend();
    renderWithProviders(<StyleCards />);
    // A 9:16 project turns on in Bold (the formats' default): picking it writes no style, the model does.
    await userEvent.click(screen.getByRole('radio', { name: 'Bold pop' }));
    const tx = (await landed()).at(-1);
    expect(tx.label).toBe('captions.on.undo');
    expect(tx.ops).toEqual([
      expect.objectContaining({ type: 'setCaptions', captions: expect.objectContaining({ enabled: true }) }),
    ]);
    expect(tx.ops[0].captions.style ?? null).toBeNull();
  });

  it('previews a style on hover and applies it on click', async () => {
    backend();
    renderWithProviders(<StyleCards />);
    const karaoke = screen.getByRole('radio', { name: 'Karaoke' });
    await userEvent.hover(karaoke);
    await waitFor(() =>
      expect(
        calls.some((c) => c.cmd === 'engine_preview' && JSON.stringify(c.args.ops).includes('caption.karaoke')),
      ).toBe(true),
    );
    await userEvent.click(karaoke);
    const tx = (await landed()).at(-1);
    expect(tx.label).toBe('captions.style.undo');
    expect(tx.ops[0].captions).toEqual(
      expect.objectContaining({ style: 'klipolo.builtin/caption.karaoke', enabled: true }),
    );
  });

  it('leaves the chosen style as it is: no preview, no change', async () => {
    backend();
    const snap = snapshot(true);
    snap.project.captions = { ...snap.project.captions, style: 'klipolo.builtin/caption.bold', accent: '#39E508' };
    useProject.setState({ snapshot: snap });
    renderWithProviders(<StyleCards />);
    const bold = screen.getByRole('radio', { name: 'Bold pop' });
    expect(bold).toHaveAttribute('aria-checked', 'true');
    await userEvent.hover(bold);
    await userEvent.click(bold);
    // Another card's preview lands, so a frame has passed since the chosen one's hover and click.
    await userEvent.hover(screen.getByRole('radio', { name: 'Boxed' }));
    await waitFor(() => expect(callsOf('engine_preview').length).toBeGreaterThan(0));
    expect(callsOf('engine_preview').every((c) => !JSON.stringify(c.args.ops).includes('caption.bold"'))).toBe(
      true,
    );
    expect(applied()).toEqual([]);
  });

  it('turns captions on from the chosen style while they are off, keeping its look and color', async () => {
    backend();
    const snap = snapshot(false);
    snap.project.captions = { ...snap.project.captions, style: 'klipolo.builtin/caption.boxed', accent: '#39E508' };
    useProject.setState({ snapshot: snap });
    renderWithProviders(<StyleCards />);
    await userEvent.click(screen.getByRole('radio', { name: 'Boxed' }));
    const tx = (await landed()).at(-1);
    expect(tx.label).toBe('captions.on.undo');
    expect(tx.ops[0].captions).toEqual(
      expect.objectContaining({ enabled: true, style: 'klipolo.builtin/caption.boxed', accent: '#39E508' }),
    );
  });

  it('Off turns captions off as one undo step, keeping the style for next time', async () => {
    backend();
    const snap = snapshot(true);
    snap.project.captions = { ...snap.project.captions, style: 'klipolo.builtin/caption.karaoke' };
    useProject.setState({ snapshot: snap });
    renderWithProviders(<StyleCards />);
    expect(screen.getByRole('radio', { name: 'Karaoke' })).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(screen.getByRole('radio', { name: 'Off' }));
    const tx = (await landed()).at(-1);
    expect(tx.label).toBe('captions.off.undo');
    expect(tx.ops[0].captions).toEqual(
      expect.objectContaining({ enabled: false, style: 'klipolo.builtin/caption.karaoke' }),
    );
  });
});

/** jsdom's own `offsetTop`, put back after a test stubs it. */
const offsetTop = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetTop')!;
afterEach(() => void Object.defineProperty(HTMLElement.prototype, 'offsetTop', offsetTop));

describe('the style cards’ keys and marks', () => {
  it('arrow keys follow the measured columns', async () => {
    backend();
    Object.defineProperty(HTMLElement.prototype, 'offsetTop', {
      configurable: true,
      get(this: HTMLElement) {
        return Math.floor([...(this.parentElement?.children ?? [])].indexOf(this) / 3) * 80;
      },
    });
    renderWithProviders(<StyleCards />);
    const all = cards();
    act(() => all[0].focus());
    await userEvent.keyboard('{ArrowDown}');
    expect(all[3]).toHaveFocus();
    await userEvent.keyboard('{ArrowUp}{ArrowRight}');
    expect(all[1]).toHaveFocus();
  });

  it('the chosen card shows a check, not only a color', () => {
    backend();
    useProject.setState({ snapshot: snapshot(true) });
    renderWithProviders(<StyleCards />);
    const chosen = within(screen.getByRole('radiogroup', { name: 'Caption style' })).getByRole('radio', {
      checked: true,
    });
    expect(chosen).toHaveAttribute('aria-label', 'Bold pop');
    expect(within(chosen).getByTestId('style-chosen')).toBeInTheDocument();
  });
});

describe('the Captions view in the left panel', () => {
  it('opens from its tab with the styles and the words, and gives the Library back', async () => {
    backend();
    useProject.setState({ snapshot: snapshot(true) });
    renderWithProviders(<Library />);
    await userEvent.click(screen.getByRole('tab', { name: 'Captions' }));
    expect(screen.getByRole('tab', { name: 'Captions' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('radiogroup', { name: 'Caption style' })).toBeInTheDocument();
    expect(await screen.findByRole('listbox', { name: 'Caption words' })).toBeInTheDocument();
    expect(screen.getByRole('complementary', { name: 'Captions' })).toHaveAttribute('data-view', 'captions');
    await userEvent.click(screen.getByRole('tab', { name: 'Library' }));
    expect(screen.queryByRole('radiogroup', { name: 'Caption style' })).toBeNull();
    expect(screen.getByRole('tab', { name: 'Clips' })).toBeInTheDocument();
  });
});
