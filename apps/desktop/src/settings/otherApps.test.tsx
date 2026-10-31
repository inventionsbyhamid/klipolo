import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { act, render, screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { McpStatus } from '../bindings/McpStatus';

vi.mock('../bridge/ai', () => ({
  aiBridge: { status: vi.fn(), setKey: vi.fn(), setCode: vi.fn(), clear: vi.fn(), onStatus: vi.fn() },
}));
vi.mock('../bridge/mcp', () => ({
  mcpStatus: vi.fn(),
  setMcpEnabled: vi.fn(),
  onMcpApps: vi.fn(),
  onMcpReply: vi.fn(),
  sendEditorView: vi.fn(),
}));
const { aiBridge } = await import('../bridge/ai');
const mcp = await import('../bridge/mcp');
const { useAi } = await import('../ai/aiStatus');
const { SettingsDialog } = await import('./SettingsDialog');

const CONFIG = JSON.stringify(
  { mcpServers: { klipolo: { command: '/Applications/Klipolo.app/Contents/MacOS/klipolo', args: ['--mcp'] } } },
  null,
  2,
);
const off: McpStatus = { enabled: false, connected: [], config: CONFIG, needsMove: false };
const on: McpStatus = { ...off, enabled: true };

/** What `mcp-apps` would tell, once the section listens. */
let tellApps: (apps: string[]) => void = () => undefined;

/** The Settings dialog open, Rust saying `status` about other apps. */
async function openOn(status: McpStatus) {
  vi.mocked(mcp.mcpStatus).mockResolvedValue(status);
  useAi.setState({ status: { mode: 'key', keyHint: '6789' }, settingsOpen: true });
  render(<SettingsDialog />);
  const section = await screen.findByRole('region', { name: 'Other apps' });
  // Rust's answer is in.
  await act(async () => undefined);
  return section;
}

const toggle = () => screen.getByRole('switch', { name: 'Let other apps edit with Klipolo' });

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(aiBridge.status).mockResolvedValue({ mode: 'key', keyHint: '6789' });
  vi.mocked(mcp.onMcpApps).mockImplementation(async (cb) => {
    tellApps = cb;
    return () => undefined;
  });
  useAi.setState({ status: null, settingsOpen: false });
});

describe('Settings: Other apps', () => {
  it('switch_turns_listening_on_and_off', async () => {
    const section = await openOn(off);
    expect(within(section).getByRole('heading', { name: 'Other apps' })).toBeInTheDocument();
    expect(section).toHaveTextContent(
      'AI agents and other MCP apps can edit the open project. Cuts always wait for your Apply.',
    );
    expect(toggle()).toHaveAttribute('aria-checked', 'false');
    expect(section).toHaveTextContent('Off.');

    vi.mocked(mcp.setMcpEnabled).mockResolvedValueOnce(on);
    await userEvent.click(toggle());
    expect(mcp.setMcpEnabled).toHaveBeenLastCalledWith(true);
    expect(toggle()).toHaveAttribute('aria-checked', 'true');
    expect(section).toHaveTextContent('On. No app connected yet.');

    vi.mocked(mcp.setMcpEnabled).mockResolvedValueOnce(off);
    await userEvent.click(toggle());
    expect(mcp.setMcpEnabled).toHaveBeenLastCalledWith(false);
    expect(toggle()).toHaveAttribute('aria-checked', 'false');
    expect(section).toHaveTextContent('Off.');
  });

  it('a_start_that_fails_says_why_and_stays_off', async () => {
    const section = await openOn(off);
    vi.mocked(mcp.setMcpEnabled).mockRejectedValueOnce(
      new Error("Klipolo couldn't start listening for other apps. Try again."),
    );
    await userEvent.click(toggle());
    expect(within(section).getByRole('alert')).toHaveTextContent(
      "Klipolo couldn't start listening for other apps. Try again.",
    );
    expect(toggle()).toHaveAttribute('aria-checked', 'false');
    expect(section).toHaveTextContent('Off.');
    // Tried again and started: the sentence goes.
    vi.mocked(mcp.setMcpEnabled).mockResolvedValueOnce(on);
    await userEvent.click(toggle());
    expect(within(section).queryByRole('alert')).toBeNull();
    expect(toggle()).toHaveAttribute('aria-checked', 'true');
  });

  it('copy_config_puts_the_json_on_the_clipboard', async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: write }, configurable: true });
    const section = await openOn(on);
    await userEvent.click(within(section).getByRole('button', { name: 'Copy config' }));
    expect(write).toHaveBeenCalledWith(CONFIG);
    expect(section).toHaveTextContent("Copied. Paste it into your app's MCP settings, then restart that app.");
  });

  it('translocated_app_asks_to_move_first', async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: write }, configurable: true });
    const section = await openOn({ ...off, needsMove: true });
    const copy = within(section).getByRole('button', { name: 'Copy config' });
    expect(copy).toBeDisabled();
    expect(section).toHaveTextContent('Move Klipolo to your Applications folder first, then copy the config.');
    await userEvent.click(copy);
    expect(write).not.toHaveBeenCalled();
    // The switch still works: only the config can't name this copy of Klipolo.
    expect(toggle()).not.toHaveAttribute('aria-disabled');
  });

  it('connected_apps_are_listed', async () => {
    const section = await openOn({ ...on, connected: ['Example Client', 'Muse Spark agent'] });
    expect(section).toHaveTextContent('On. Recently used by Example Client and Cursor.');
    act(() => tellApps(['Muse Spark agent']));
    expect(section).toHaveTextContent('On. Recently used by Muse Spark agent.');
    act(() => tellApps([]));
    expect(section).toHaveTextContent('On. No app connected yet.');
  });

  it('page_message_ids_stay_out_of_rusts_mcp_namespace', () => {
    // `mcp.` is the namespace of Rust's codes (`mcp.fromEarlier`, a label Klipolo's host gives): the page's own words
    // use their own ids, so one id is never two messages.
    for (const file of [
      'OtherApps.tsx',
      '../state/chatApps.ts',
      '../features/chat/ChatMessage.tsx',
      '../features/chat/ProposalCard.tsx',
      '../features/chat/Announcer.tsx',
      '../lib/labelTable.ts',
    ]) {
      const ids = [...readFileSync(join(__dirname, file), 'utf8').matchAll(/id: '(mcp\.[\w.]+)'/g)].map((m) => m[1]);
      expect(ids.filter((id) => id !== 'mcp.fromEarlier'), file).toEqual([]);
    }
  });

  it('no_em_or_en_dash_in_new_copy', async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: write }, configurable: true });
    const section = await openOn({ ...on, connected: ['Example Client'], needsMove: true });
    expect(section.textContent).not.toMatch(/[–—]/);
    // Every sentence the new code can say, in its source: the section, the bridge, chat's app messages and labels.
    for (const file of [
      'OtherApps.tsx',
      '../bridge/mcp.ts',
      '../state/chatApps.ts',
      '../state/editorView.ts',
      '../features/chat/ChatMessage.tsx',
      '../features/chat/ProposalCard.tsx',
      '../features/chat/Announcer.tsx',
      '../features/chat/UndoChip.tsx',
      '../lib/labelTable.ts',
      '../lib/labels.ts',
    ]) {
      expect(readFileSync(join(__dirname, file), 'utf8'), file).not.toMatch(/[–—]/);
    }
  });
});
