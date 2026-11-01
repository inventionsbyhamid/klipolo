import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AiStatus } from '../bindings/AiStatus';

vi.mock('../bridge/ai', () => ({
  aiBridge: { status: vi.fn(), setKey: vi.fn(), setCode: vi.fn(), clear: vi.fn(), onStatus: vi.fn() },
}));
const { aiBridge } = await import('../bridge/ai');
const { useAi } = await import('../ai/aiStatus');
const { SettingsDialog } = await import('./SettingsDialog');
const { useUi } = await import('../state/ui');

const KEY = 'sk-meta-test-key-0123456789';
const relay: AiStatus = { mode: 'relay', code: 'KLIP-TEST-0001', budgetUsd: 2, spentUsd: 0.06, active: true };

/** The dialog open on `status`; the status Rust answers when the dialog asks again is the same. */
function openOn(status: AiStatus) {
  useAi.setState({ status, settingsOpen: true });
  vi.mocked(aiBridge.status).mockResolvedValue(status);
  return render(<SettingsDialog />);
}

beforeEach(() => {
  vi.clearAllMocks();
  useAi.setState({ status: null, settingsOpen: false });
});

describe('SettingsDialog', () => {
  it('opens with Command comma and closes with Esc, giving focus back', async () => {
    vi.mocked(aiBridge.status).mockResolvedValue({ mode: 'none' });
    useAi.setState({ status: { mode: 'none' } });
    render(
      <>
        <button>Before</button>
        <SettingsDialog />
      </>,
    );
    const before = screen.getByRole('button', { name: 'Before' });
    before.focus();
    fireEvent.keyDown(window, { key: ',', metaKey: true });
    const dialog = await screen.findByRole('dialog', { name: 'Settings' });
    expect(dialog).toHaveTextContent("Chat, transcripts, captions and stickers use Meta's models.");
    expect(screen.getByRole('radio', { name: /Access code/ })).toHaveAttribute('aria-checked', 'true');
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(before).toHaveFocus();
  });

  it('has Appearance: System, Light or Dark, System until one is picked', async () => {
    openOn(relay);
    const section = screen.getByRole('region', { name: 'Appearance' });
    const option = (name: string) => within(section).getByRole('radio', { name });
    expect(option('System')).toHaveAttribute('aria-checked', 'true');
    expect(option('Light')).toHaveAttribute('aria-checked', 'false');
    expect(option('Dark')).toHaveAttribute('aria-checked', 'false');
    await userEvent.click(option('Light'));
    expect(useUi.getState().theme).toBe('light');
    expect(option('Light')).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(option('System'));
    expect(useUi.getState().theme).toBe('system');
  });

  it('has the editing copies switch, so copies turned off can be turned on again', async () => {
    openOn(relay);
    const section = screen.getByRole('region', { name: 'Editing copies' });
    expect(within(section).getByRole('switch', { name: 'Make editing copies automatically' })).toBeInTheDocument();
    expect(section).toHaveTextContent('Exports always use your full-quality files.');
    await act(async () => undefined);
  });

  it('shows a saved key only as its last four characters', async () => {
    openOn({ mode: 'key', keyHint: '6789' });
    expect(screen.getByRole('radio', { name: /My Meta API key/ })).toHaveAttribute('aria-checked', 'true');
    // A read-only field named by its label, as the code's is; it never holds the key.
    const field = screen.getByLabelText('Meta API key');
    expect(field).toHaveValue('Saved, ending in 6789');
    expect(field).toHaveAttribute('readonly');
    expect(screen.getByText("Kept in your Mac's Keychain.")).toBeInTheDocument();
    expect(document.body.textContent).not.toContain(KEY);
    expect(screen.queryByDisplayValue(KEY)).toBeNull();
    await act(async () => undefined);
  });

  it('checks a key before saving it, and says why when Meta refuses it', async () => {
    openOn({ mode: 'none' });
    await userEvent.click(screen.getByRole('radio', { name: /My Meta API key/ }));
    const field = screen.getByLabelText('Meta API key');
    expect(field).toHaveAttribute('type', 'password');
    await userEvent.type(field, KEY);
    let refuse: (error: Error) => void = () => undefined;
    vi.mocked(aiBridge.setKey).mockReturnValue(new Promise((_, reject) => (refuse = reject)));
    await userEvent.click(screen.getByRole('button', { name: 'Save key' }));
    expect(aiBridge.setKey).toHaveBeenCalledWith(KEY);
    expect(screen.getByRole('button', { name: 'Checking' })).toBeDisabled();
    await act(async () => refuse(new Error("That key didn't work. Check it on dev.meta.ai.")));
    expect(screen.getByRole('alert')).toHaveTextContent("That key didn't work. Check it on dev.meta.ai.");
    expect(field).toHaveAttribute('aria-invalid', 'true');
    expect(useAi.getState().status).toEqual({ mode: 'none' });
  });

  it('saves a good key and then shows only that it is saved', async () => {
    openOn({ mode: 'none' });
    await userEvent.click(screen.getByRole('radio', { name: /My Meta API key/ }));
    await userEvent.type(screen.getByLabelText('Meta API key'), KEY);
    vi.mocked(aiBridge.setKey).mockResolvedValue({ mode: 'key', keyHint: '6789' });
    await userEvent.click(screen.getByRole('button', { name: 'Save key' }));
    expect(await screen.findByDisplayValue('Saved, ending in 6789')).toHaveAttribute('readonly');
    expect(screen.queryByDisplayValue(KEY)).toBeNull();
    // The form went with the save; focus stays in the dialog, on the card in use (not Remove: a second Enter mustn't
    // take the key away again).
    expect(screen.getByRole('radio', { name: /My Meta API key/ })).toHaveFocus();
  });

  it("shows what AI cost this session, with a key or a code", async () => {
    const session = { analysisUsd: 0.08, chatUsd: 0.02, chatMessages: 4, picturesUsd: 0, pictures: 0 };
    openOn({ mode: 'key', keyHint: '6789', session });
    const spend = screen.getByRole('group', { name: 'This session' });
    expect(spend).toHaveTextContent('This session$0.10');
    expect(spend).toHaveTextContent('Analysis $0.08, Chat $0.02 for 4 messages');
    await act(async () => undefined);
  });

  it("shows the code's credit, never more spent than the budget", async () => {
    openOn({ ...relay, spentUsd: 2.31 });
    expect(screen.getByDisplayValue('KLIP-TEST-0001')).toHaveAttribute('readonly');
    expect(screen.getByText('Active on this Mac')).toBeInTheDocument();
    const meter = screen.getByRole('meter', { name: 'Credit left' });
    expect(meter).toHaveAttribute('aria-valuenow', '0');
    expect(meter).toHaveAttribute('aria-valuetext', '$0.00 left of $2.00');
    expect(screen.getByText('left of $2.00')).toBeInTheDocument();
    expect(screen.getByText('Spent so far').nextElementSibling).toHaveTextContent('$2.00');
    await act(async () => undefined);
  });

  it('says when a code was turned off', async () => {
    openOn({
      mode: 'relay',
      code: 'KLIP-TEST-0001',
      budgetUsd: 0,
      spentUsd: 1.2,
      active: false,
      notice: 'This access code is no longer active. Enter another code or add your own Meta key.',
    });
    expect(screen.getByText(/no longer active/)).toBeInTheDocument();
    expect(screen.queryByText('Active on this Mac')).toBeNull();
    await act(async () => undefined);
  });

  it('enters a code, and Remove takes it away again', async () => {
    openOn({ mode: 'none' });
    vi.mocked(aiBridge.setCode).mockResolvedValue(relay);
    await userEvent.type(screen.getByLabelText('Access code'), 'KLIP-TEST-0001');
    await userEvent.click(screen.getByRole('button', { name: 'Use code' }));
    expect(aiBridge.setCode).toHaveBeenCalledWith('KLIP-TEST-0001');
    expect(await screen.findByText('$1.94')).toBeInTheDocument();

    vi.mocked(aiBridge.clear).mockResolvedValue({ mode: 'none' });
    await userEvent.click(screen.getByRole('button', { name: 'Remove' }));
    expect(aiBridge.clear).toHaveBeenCalledTimes(1);
    expect(await screen.findByLabelText('Access code')).not.toHaveAttribute('readonly');
    expect(screen.getByLabelText('Access code')).toHaveFocus();
  });

  it("keeps the editor's shortcuts from acting behind it", async () => {
    openOn({ mode: 'none' });
    const editor = vi.fn();
    window.addEventListener('keydown', editor);
    try {
      // On one of the dialog's own buttons.
      fireEvent.keyDown(screen.getByRole('radio', { name: /Access code/ }), { key: 'l' });
      // With focus lost to the page (WebKit doesn't focus a clicked button, so a field that goes away takes focus
      // with it): the key is kept from the editor and focus comes back into the dialog.
      (document.activeElement as HTMLElement | null)?.blur();
      fireEvent.keyDown(document.body, { key: 'k' });
      expect(editor).not.toHaveBeenCalled();
      expect(screen.getByRole('dialog')).toContainElement(document.activeElement as HTMLElement);
    } finally {
      window.removeEventListener('keydown', editor);
    }
  });

  it('opens with the cursor in the empty field, or on the way to pay in use', async () => {
    const { unmount } = openOn({ mode: 'none' });
    expect(screen.getByLabelText('Access code')).toHaveFocus();
    unmount();
    openOn({ mode: 'key', keyHint: '6789' });
    expect(screen.getByRole('radio', { name: /My Meta API key/ })).toHaveFocus();
    await act(async () => undefined);
  });

  it('puts the cursor in the field of the way to pay that was clicked', async () => {
    openOn({ mode: 'none' });
    // WebKit doesn't focus a clicked button: focus stays where it was (here the other card).
    screen.getByRole('radio', { name: /Access code/ }).focus();
    fireEvent.click(screen.getByRole('radio', { name: /My Meta API key/ }));
    expect(screen.getByLabelText('Meta API key')).toHaveFocus();
    // Or the field that went away with the swap took it.
    (document.activeElement as HTMLElement | null)?.blur();
    fireEvent.click(screen.getByRole('radio', { name: /Access code/ }));
    expect(screen.getByLabelText('Access code')).toHaveFocus();
  });

  it('moves between the two ways to pay with the arrow keys', async () => {
    openOn({ mode: 'none' });
    const code = screen.getByRole('radio', { name: /Access code/ });
    code.focus();
    await userEvent.keyboard('{ArrowRight}');
    const key = screen.getByRole('radio', { name: /My Meta API key/ });
    expect(key).toHaveAttribute('aria-checked', 'true');
    expect(key).toHaveFocus();
  });
});
