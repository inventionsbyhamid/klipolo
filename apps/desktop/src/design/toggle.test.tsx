import { render, screen } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it } from 'vitest';
import { Toggle } from './Toggle';

/** A toggle with its label beside it, as a settings row has it. */
function Row({ start = false, disabled = false }: { start?: boolean; disabled?: boolean }) {
  const [on, setOn] = useState(start);
  return (
    <div>
      <Toggle id="mirror" checked={on} onChange={setOn} disabled={disabled} />
      <label htmlFor="mirror">Mirror my camera</label>
    </div>
  );
}

describe('Toggle', () => {
  it('toggle_is_a_switch_named_by_its_label', () => {
    render(<Row start />);
    const toggle = screen.getByRole('switch', { name: 'Mirror my camera' });
    expect(toggle).toHaveAttribute('aria-checked', 'true');
    expect(toggle).toHaveClass('toggle');
  });

  it('toggle_flips_on_a_click_on_it_or_its_label_and_with_space', async () => {
    render(<Row />);
    const toggle = screen.getByRole('switch', { name: 'Mirror my camera' });
    expect(toggle).toHaveAttribute('aria-checked', 'false');
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(screen.getByText('Mirror my camera'));
    expect(toggle).toHaveAttribute('aria-checked', 'false');
    toggle.focus();
    await userEvent.keyboard(' ');
    expect(toggle).toHaveAttribute('aria-checked', 'true');
  });

  it('a_disabled_toggle_stays_as_it_is', async () => {
    render(<Row disabled />);
    const toggle = screen.getByRole('switch', { name: 'Mirror my camera' });
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-checked', 'false');
    expect(toggle).toBeDisabled();
  });
});
