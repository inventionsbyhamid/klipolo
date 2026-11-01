import { act, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { renderWithProviders } from '../../test/render';
import { ColorField, type ColorFieldProps } from './ColorField';

const PRESETS = [
  { value: '#FFFFFF', name: 'White' },
  { value: '#121A33', name: 'Navy' },
  { value: '#FFD02E', name: 'Yellow' },
  { value: '#7F92DA', name: 'Blue' },
  { value: '#000000', name: 'Black' },
];

function handlers() {
  return { onPreview: vi.fn(), onPreviewEnd: vi.fn(), onCommit: vi.fn() };
}
const field = (h: ReturnType<typeof handlers>, props: Partial<ColorFieldProps> = {}) =>
  renderWithProviders(<ColorField label="Text color" value="#FFFFFF" presets={PRESETS} {...h} {...props} />);
const row = (name = 'Text color') => screen.getByRole('radiogroup', { name });
const swatch = (name: string, group = 'Text color') => within(row(group)).getByRole('radio', { name });

afterEach(() => vi.useRealTimers());

describe('the color field', () => {
  it('hovering a swatch dry-runs it; leaving the row or Escape puts the video back', async () => {
    const h = handlers();
    field(h);
    await userEvent.hover(swatch('Yellow'));
    expect(h.onPreview).toHaveBeenLastCalledWith('#FFD02E');
    await userEvent.unhover(row());
    expect(h.onPreviewEnd).toHaveBeenCalled();
    const ends = h.onPreviewEnd.mock.calls.length;
    act(() => swatch('White').focus());
    fireEvent.keyDown(swatch('White'), { key: 'Escape' });
    expect(h.onPreviewEnd).toHaveBeenCalledTimes(ends + 1);
    expect(h.onCommit).not.toHaveBeenCalled();
  });

  it('a click is one commit; picking the colour it has commits nothing', async () => {
    const h = handlers();
    field(h);
    await userEvent.click(swatch('Yellow'));
    expect(h.onCommit).toHaveBeenCalledExactlyOnceWith('#FFD02E');
    await userEvent.click(swatch('White'));
    expect(h.onCommit).toHaveBeenCalledTimes(1);
  });

  it('a run of arrow keys dry-runs each colour and is one commit once the keys stop', () => {
    vi.useFakeTimers();
    const h = handlers();
    field(h);
    act(() => swatch('White').focus());
    for (let i = 0; i < 2; i++) fireEvent.keyDown(document.activeElement!, { key: 'ArrowRight' });
    expect(swatch('Yellow')).toHaveAttribute('aria-checked', 'true');
    expect(h.onPreview).toHaveBeenLastCalledWith('#FFD02E');
    expect(h.onCommit).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(400));
    expect(h.onCommit).toHaveBeenCalledExactlyOnceWith('#FFD02E');
  });

  it('a none swatch comes first, named, and checked while there is no colour', async () => {
    const h = handlers();
    field(h, { label: 'Outline', value: null, none: { label: 'No outline' } });
    expect(within(row('Outline')).getAllByRole('radio')[0]).toHaveAccessibleName('No outline');
    expect(swatch('No outline', 'Outline')).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(swatch('Black', 'Outline'));
    expect(h.onCommit).toHaveBeenLastCalledWith('#000000');
  });

  it('picking none commits null', async () => {
    const h = handlers();
    field(h, { label: 'Outline', value: '#000000', none: { label: 'No outline' } });
    await userEvent.click(swatch('No outline', 'Outline'));
    expect(h.onCommit).toHaveBeenCalledExactlyOnceWith(null);
  });

  it('a custom colour checks no swatch and names itself on the “+” button', () => {
    field(handlers(), { value: '#123456' });
    expect(within(row()).queryByRole('radio', { checked: true })).toBeNull();
    expect(screen.getByRole('button', { name: 'Text color: Navy, more colors' })).toBeInTheDocument();
  });

  it('with an alpha, a swatch is checked by its colour alone and a swatch pick keeps the alpha', async () => {
    const h = handlers();
    field(h, { label: 'Highlight', value: '#121A33DB', alpha: true, none: { label: 'No highlight' } });
    expect(swatch('Navy', 'Highlight')).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(swatch('Yellow', 'Highlight'));
    expect(h.onCommit).toHaveBeenCalledExactlyOnceWith('#FFD02EDB');
  });

  it('the hex field takes three or six digits, refuses others and says why, and drops an opacity it can’t keep', async () => {
    const h = handlers();
    field(h);
    await userEvent.click(screen.getByRole('button', { name: 'Text color: more colors' }));
    const hex = screen.getByRole('textbox', { name: 'Hex color' });
    expect(hex).toHaveValue('FFFFFF');
    await userEvent.clear(hex);
    await userEvent.paste('ff0');
    await userEvent.keyboard('{Enter}');
    expect(h.onCommit).toHaveBeenCalledExactlyOnceWith('#FFFF00');
    await userEvent.clear(hex);
    await userEvent.type(hex, 'red{Enter}');
    expect(hex).toHaveAttribute('aria-invalid', 'true');
    expect(hex).toHaveAccessibleDescription('Type 3 or 6 hex digits, like FFD02E.');
    expect(h.onCommit).toHaveBeenCalledTimes(1);
    await userEvent.clear(hex);
    await userEvent.paste('#FFD02E80');
    await userEvent.keyboard('{Enter}');
    expect(h.onCommit).toHaveBeenLastCalledWith('#FFD02E');
    expect(hex).toHaveAccessibleDescription('This setting has no opacity, so the color is used without it.');
  });
});
