import { render, screen } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../bridge/ai', () => ({
  aiBridge: { status: vi.fn(), setKey: vi.fn(), setCode: vi.fn(), clear: vi.fn(), onStatus: vi.fn() },
}));
const { useAi } = await import('./aiStatus');
const { NeedsKey } = await import('./NeedsKey');

beforeEach(() => useAi.setState({ status: null, settingsOpen: false }));

describe('NeedsKey', () => {
  it('asks for a key or code when there is neither, and opens Settings', async () => {
    useAi.setState({ status: { mode: 'none' } });
    render(<NeedsKey />);
    expect(screen.getByText('Add your Meta API key or access code in Settings')).toBeInTheDocument();
    expect(
      screen.getByText("Chat, transcripts and captions use Meta's models. Editing and export work without them."),
    ).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Open Settings' }));
    expect(useAi.getState().settingsOpen).toBe(true);
  });

  it('stays away with a key, a code, or before the status is known', () => {
    const { container, rerender } = render(<NeedsKey />);
    expect(container).toBeEmptyDOMElement();
    useAi.setState({ status: { mode: 'key', keyHint: '6789' } });
    rerender(<NeedsKey />);
    expect(container).toBeEmptyDOMElement();
    useAi.setState({ status: { mode: 'relay', code: 'KLIP-TEST-0001' } });
    rerender(<NeedsKey />);
    expect(container).toBeEmptyDOMElement();
  });
});
