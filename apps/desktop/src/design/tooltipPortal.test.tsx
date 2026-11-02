import { act, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { renderWithProviders } from '../test/render';

/** Radix's tooltip portal, counted as it draws: a closed tooltip's would draw nothing, so it isn't there at all. */
const portals = vi.hoisted(() => ({ n: 0 }));
vi.mock('radix-ui', async (original) => {
  const real = await original<typeof import('radix-ui')>();
  const Portal = (props: Parameters<typeof real.Tooltip.Portal>[0]) => {
    portals.n += 1;
    return <real.Tooltip.Portal {...props} />;
  };
  return { ...real, Tooltip: { ...real.Tooltip, Portal } };
});
const { IconButton } = await import('./IconButton');
const { Scissors } = await import('lucide-react');

describe('a tooltip', () => {
  it('draws no portal while closed, and its label once open', async () => {
    renderWithProviders(
      <>
        {Array.from({ length: 20 }, (_, i) => (
          <IconButton key={i} icon={Scissors} label={`Split ${i}`} />
        ))}
      </>,
    );
    expect(portals.n).toBe(0);
    act(() => screen.getByRole('button', { name: 'Split 3' }).focus());
    expect(await screen.findByRole('tooltip')).toHaveTextContent('Split 3');
    expect(portals.n).toBeGreaterThan(0);
  });
});
