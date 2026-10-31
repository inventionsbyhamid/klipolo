import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AiStatus } from '../bindings/AiStatus';

vi.mock('../bridge/ai', () => ({
  aiBridge: { status: vi.fn(), setKey: vi.fn(), setCode: vi.fn(), clear: vi.fn(), onStatus: vi.fn() },
}));
const { aiBridge } = await import('../bridge/ai');
const { useAi, needsKey } = await import('../ai/aiStatus');
const { budgetView, chipView, initialChoice, removePayment, saveCode, saveKey, sessionView, usd } = await import(
  './settings'
);

const KEY = 'sk-meta-test-key-0123456789';
const relay: AiStatus = { mode: 'relay', code: 'KLIP-TEST-0001', budgetUsd: 2, spentUsd: 0.06, active: true };

beforeEach(() => {
  vi.clearAllMocks();
  useAi.setState({ status: null, settingsOpen: false });
});

describe("the session's AI spend", () => {
  const spent = { analysisUsd: 0.08, chatUsd: 0.021, chatMessages: 4, picturesUsd: 0.02, pictures: 2 };

  it('adds up what analysis, chat and pictures cost, each kind named', () => {
    expect(sessionView({ mode: 'key', keyHint: '6789', session: spent })).toEqual({
      total: '$0.12',
      kinds: ['Analysis $0.08', 'Chat $0.02 for 4 messages', 'Pictures $0.02 for 2'],
    });
  });

  it('leaves out what cost nothing, and says less than a cent', () => {
    const session = { analysisUsd: 0, chatUsd: 0.004, chatMessages: 1, picturesUsd: 0, pictures: 0 };
    expect(sessionView({ ...relay, session })).toEqual({
      total: 'under $0.01',
      kinds: ['Chat under $0.01 for 1 message'],
    });
  });

  it('shows nothing before anything was spent', () => {
    const none = { analysisUsd: 0, chatUsd: 0, chatMessages: 0, picturesUsd: 0, pictures: 0 };
    expect(sessionView({ mode: 'key', session: none })).toBeNull();
    expect(sessionView({ mode: 'key' })).toBeNull();
    expect(sessionView(null)).toBeNull();
  });
});

describe('the budget', () => {
  it('shows what is left of the code’s credit', () => {
    expect(budgetView(relay)).toEqual({ budget: 2, spent: 0.06, left: 1.94, shareLeft: 0.97 });
  });

  it('never shows more spent than the budget, nor less than nothing', () => {
    expect(budgetView({ ...relay, spentUsd: 2.31 })).toEqual({ budget: 2, spent: 2, left: 0, shareLeft: 0 });
    expect(budgetView({ ...relay, spentUsd: -1 })).toEqual({ budget: 2, spent: 0, left: 2, shareLeft: 1 });
    expect(budgetView({ ...relay, budgetUsd: 0, spentUsd: 0 })).toEqual({
      budget: 0,
      spent: 0,
      left: 0,
      shareLeft: 0,
    });
  });

  it('is unknown until the relay says', () => {
    expect(budgetView({ mode: 'relay', code: 'KLIP-TEST-0001' })).toBeNull();
    expect(budgetView({ mode: 'key', keyHint: '6789' })).toBeNull();
  });

  it('is written in dollars and cents', () => {
    expect(usd(1.94)).toBe('$1.94');
    expect(usd(0)).toBe('$0.00');
    expect(usd(2)).toBe('$2.00');
  });
});

describe('the top bar chip', () => {
  it('asks for a key, shows the code’s credit, or names the key', () => {
    expect(chipView(null)).toBeNull();
    expect(chipView({ mode: 'none' })).toEqual({ icon: 'key', label: 'Add Meta key', missing: true });
    expect(chipView(relay)).toEqual({ icon: 'ticket', label: 'Access code', left: '$1.94', missing: false });
    expect(chipView({ mode: 'relay', code: 'KLIP-TEST-0001' })).toEqual({
      icon: 'ticket',
      label: 'Access code',
      missing: false,
    });
    expect(chipView({ mode: 'key', keyHint: '6789' })).toEqual({ icon: 'key', label: 'Meta key', missing: false });
  });
});

describe('choosing how to pay', () => {
  it('opens on what is in use, and on the access code when nothing is', () => {
    expect(initialChoice({ mode: 'key', keyHint: '6789' })).toBe('key');
    expect(initialChoice(relay)).toBe('code');
    expect(initialChoice({ mode: 'none' })).toBe('code');
    expect(initialChoice(null)).toBe('code');
  });

  it('needs a key only when there is neither a key nor a code', () => {
    expect(needsKey({ mode: 'none' })).toBe(true);
    expect(needsKey(relay)).toBe(false);
    expect(needsKey(null)).toBe(false);
  });
});

describe('saving', () => {
  it('sends the key to Rust and keeps only the status it answers', async () => {
    vi.mocked(aiBridge.setKey).mockResolvedValue({ mode: 'key', keyHint: '6789' });
    expect(await saveKey(KEY)).toBeNull();
    expect(aiBridge.setKey).toHaveBeenCalledWith(KEY);
    expect(useAi.getState().status).toEqual({ mode: 'key', keyHint: '6789' });
    expect(JSON.stringify(useAi.getState())).not.toContain(KEY);
  });

  it('says why a key was not saved and keeps the status', async () => {
    useAi.setState({ status: { mode: 'none' } });
    vi.mocked(aiBridge.setKey).mockRejectedValue(new Error("That key didn't work. Check it on dev.meta.ai."));
    expect(await saveKey(KEY)).toBe("That key didn't work. Check it on dev.meta.ai.");
    expect(useAi.getState().status).toEqual({ mode: 'none' });
  });

  it('asks for a key or code before sending an empty one', async () => {
    expect(await saveKey('   ')).toBe('Paste your Meta API key first.');
    expect(await saveCode('')).toBe('Enter your access code first.');
    expect(aiBridge.setKey).not.toHaveBeenCalled();
    expect(aiBridge.setCode).not.toHaveBeenCalled();
  });

  it('registers an access code and shows its credit', async () => {
    vi.mocked(aiBridge.setCode).mockResolvedValue(relay);
    expect(await saveCode(' KLIP-TEST-0001 ')).toBeNull();
    expect(aiBridge.setCode).toHaveBeenCalledWith('KLIP-TEST-0001');
    expect(useAi.getState().status).toEqual(relay);
  });

  it('shows the relay’s own reason for a code it refused', async () => {
    const reason = 'This access code is not valid. Check the code or add your own Meta key in Settings.';
    vi.mocked(aiBridge.setCode).mockRejectedValue(new Error(reason));
    expect(await saveCode('KLIP-TEST-0009')).toBe(reason);
  });

  it('removes the key or code', async () => {
    useAi.setState({ status: relay });
    vi.mocked(aiBridge.clear).mockResolvedValue({ mode: 'none' });
    expect(await removePayment()).toBeNull();
    expect(useAi.getState().status).toEqual({ mode: 'none' });
  });
});
