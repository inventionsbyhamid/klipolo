import { act, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Scissors } from 'lucide-react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { toast, useToasts } from '../state/toasts';
import { renderWithProviders } from '../test/render';
import { Button } from './Button';
import { IconButton } from './IconButton';
import { Kbd } from './Kbd';
import { Menu, MenuCheckboxItem, MenuContent, MenuTrigger } from './Menu';
import { Toaster } from './Toaster';

afterEach(() => useToasts.setState({ toasts: [] }));

describe('design primitives', () => {
  it('buttons default to type=button and apply the variant', () => {
    renderWithProviders(<Button variant="primary">Export</Button>);
    const button = screen.getByRole('button', { name: 'Export' });
    expect(button).toHaveAttribute('type', 'button');
    expect(button).toHaveClass('btn', 'btn-primary');
  });

  it('icon buttons keep their label as the name and report pressed state', () => {
    // aria-label and aria-pressed are not props (IconButtonProps omits them): label and active own them.
    renderWithProviders(<IconButton icon={Scissors} label="Split" active />);
    expect(screen.getByRole('button', { name: 'Split' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('icon button tooltips show the label and the shortcut as a key', async () => {
    renderWithProviders(<IconButton icon={Scissors} label="Split" shortcut="S" />);
    await userEvent.tab();
    const tooltip = await screen.findByRole('tooltip');
    expect(tooltip).toHaveTextContent('Split');
    expect(within(tooltip).getByText('S').tagName).toBe('KBD');
  });

  it('kbd renders a keyboard shortcut', () => {
    renderWithProviders(<Kbd>⌘K</Kbd>);
    const key = screen.getByText('⌘K');
    expect(key.tagName).toBe('KBD');
  });

  it('menu checkbox items toggle without closing the menu', async () => {
    const onCheckedChange = vi.fn();
    renderWithProviders(
      <Menu>
        <MenuTrigger asChild>
          <Button>View</Button>
        </MenuTrigger>
        <MenuContent>
          <MenuCheckboxItem checked={false} onCheckedChange={onCheckedChange}>
            Show waveforms
          </MenuCheckboxItem>
        </MenuContent>
      </Menu>,
    );
    await userEvent.click(screen.getByRole('button', { name: 'View' }));
    await userEvent.click(screen.getByRole('menuitemcheckbox', { name: 'Show waveforms' }));
    expect(onCheckedChange).toHaveBeenCalledWith(true);
    expect(screen.getByRole('menu')).toBeInTheDocument();
  });

  it('toasts appear and can be dismissed', () => {
    renderWithProviders(<Toaster />);
    act(() => toast.error('Couldn’t save the project'));
    expect(screen.getByText('Couldn’t save the project')).toBeInTheDocument();
    act(() => useToasts.getState().dismiss(useToasts.getState().toasts[0].id));
    expect(useToasts.getState().toasts).toHaveLength(0);
    expect(screen.queryByText('Couldn’t save the project')).not.toBeInTheDocument();
  });

  it('escape closes a toast through the store', async () => {
    renderWithProviders(<Toaster />);
    act(() => toast.info('Project saved'));
    expect(screen.getByText('Project saved')).toBeInTheDocument();
    await userEvent.keyboard('{Escape}');
    expect(useToasts.getState().toasts).toHaveLength(0);
    expect(screen.queryByText('Project saved')).not.toBeInTheDocument();
  });

  it('a persistent toast closes on Escape only from inside it; plain toasts still close on Escape', async () => {
    const shown = () => useToasts.getState().toasts.map((t) => t.text);
    const item = (text: string) => screen.getByText(text).closest('li')!;
    const prevented: boolean[] = [];
    const heard = (e: KeyboardEvent) => prevented.push(e.defaultPrevented);
    window.addEventListener('keydown', heard);
    try {
      renderWithProviders(<Toaster />);
      act(() => {
        toast.info('Below');
        toast.info('Kept', { persistent: true });
        toast.info('Above');
      });
      // From elsewhere: the newest plain toast closes, then the persistent one stays.
      await userEvent.keyboard('{Escape}');
      expect(shown()).toEqual(['Below', 'Kept']);
      await userEvent.keyboard('{Escape}');
      expect(shown()).toEqual(['Below', 'Kept']);
      // The key stays the editor's (Escape clears its selection): it isn't marked handled.
      expect(prevented).toEqual([false, false]);
      // A focused plain toast below it still closes on Escape, and so does the persistent one, focused.
      act(() => item('Below').focus());
      await userEvent.keyboard('{Escape}');
      expect(shown()).toEqual(['Kept']);
      act(() => item('Kept').focus());
      await userEvent.keyboard('{Escape}');
      expect(shown()).toEqual([]);
    } finally {
      window.removeEventListener('keydown', heard);
    }
  });

  it('a menu opened before a persistent toast still closes from the keyboard', async () => {
    renderWithProviders(
      <>
        <Toaster />
        <Menu>
          <MenuTrigger asChild>
            <Button>View</Button>
          </MenuTrigger>
          <MenuContent>
            <MenuCheckboxItem checked={false} onCheckedChange={vi.fn()}>
              Show waveforms
            </MenuCheckboxItem>
          </MenuContent>
        </Menu>
      </>,
    );
    await userEvent.click(screen.getByRole('button', { name: 'View' }));
    act(() => toast.info('Kept', { persistent: true }));
    // Only Radix's topmost layer (the toast) hears Escape: the toast gives way, and the next Escape closes the menu.
    await userEvent.keyboard('{Escape}');
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  });

  it('a keyed toast said again is replaced where it shows; pressing one of its actions runs it and closes it', async () => {
    const first = vi.fn();
    const second = vi.fn();
    renderWithProviders(<Toaster />);
    act(() => {
      toast.info('Old note', { key: 'note', actions: [{ label: 'First', run: first }] });
      toast.info('Later');
      toast.info('Note again', {
        key: 'note',
        actions: [
          { label: 'First', run: first },
          { label: 'Second', run: second },
        ],
      });
    });
    expect(useToasts.getState().toasts.map((t) => t.text)).toEqual(['Note again', 'Later']);
    expect(screen.queryByText('Old note')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Second' }));
    expect(second).toHaveBeenCalledTimes(1);
    expect(first).not.toHaveBeenCalled();
    expect(useToasts.getState().toasts.map((t) => t.text)).toEqual(['Later']);
  });

  it('error toasts name their kind in text, not only in colour', () => {
    renderWithProviders(<Toaster />);
    act(() => {
      toast.error('Couldn’t save the project');
      toast.info('Project saved');
    });
    expect(screen.getByText('Couldn’t save the project')).toHaveTextContent(
      'Error: Couldn’t save the project',
    );
    expect(screen.getByText('Project saved')).not.toHaveTextContent('Error');
  });
});
