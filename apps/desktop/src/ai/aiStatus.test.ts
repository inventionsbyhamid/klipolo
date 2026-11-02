import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AiStatus } from '../bindings/AiStatus';

vi.mock('../bridge/ai', () => ({
  aiBridge: { status: vi.fn(), setKey: vi.fn(), setCode: vi.fn(), clear: vi.fn(), onStatus: vi.fn() },
}));
const { aiBridge } = await import('../bridge/ai');
const { creditMoved, followAiStatus, refreshAiStatus, setAiStatus, useAi } = await import('./aiStatus');

const relay: AiStatus = { mode: 'relay', code: 'KLIP-TEST-0001', budgetUsd: 2, spentUsd: 0.06, active: true };
const key: AiStatus = { mode: 'key', keyHint: '6789' };

/** A status answer the test hands back when it chooses. */
function heldAnswer() {
  let answer: (status: AiStatus) => void = () => undefined;
  vi.mocked(aiBridge.status).mockReturnValue(new Promise((resolve) => (answer = resolve)));
  return (status: AiStatus) => answer(status);
}

beforeEach(() => {
  vi.clearAllMocks();
  useAi.setState({ status: null, settingsOpen: false });
});

describe('the AI status', () => {
  it('ignores an answer asked for before the last change', async () => {
    const answer = heldAnswer();
    const asking = refreshAiStatus();
    // A save (or an `ai-status` event) lands while the relay is still being asked.
    setAiStatus(key);
    answer(relay);
    expect(await asking).toEqual(key);
    expect(useAi.getState().status).toEqual(key);
  });

  it('keeps an answer when nothing changed meanwhile', async () => {
    const answer = heldAnswer();
    const asking = refreshAiStatus();
    answer(relay);
    expect(await asking).toEqual(relay);
    expect(useAi.getState().status).toEqual(relay);
  });

  it('follows every change Rust tells, and an older read never undoes one', async () => {
    let tell: (status: AiStatus) => void = () => undefined;
    vi.mocked(aiBridge.onStatus).mockImplementation(async (handler) => {
      tell = handler;
      return () => undefined;
    });
    const answer = heldAnswer();
    const following = followAiStatus();
    await Promise.resolve();
    tell({ mode: 'none' });
    answer(relay);
    await following;
    expect(useAi.getState().status).toEqual({ mode: 'none' });
  });
});

describe('when a paid job ends', () => {
  const spent: AiStatus = { ...relay, spentUsd: 0.088 };
  /** Lets the asks in flight answer. */
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

  it('asks again, so the top bar and Settings show the credit left after it', async () => {
    setAiStatus(relay);
    vi.mocked(aiBridge.status).mockResolvedValue(spent);
    creditMoved();
    await settle();
    expect(aiBridge.status).toHaveBeenCalledTimes(1);
    expect(useAi.getState().status).toEqual(spent);
  });

  it("asks with a key too: Settings shows the session's spend", async () => {
    setAiStatus(key);
    const withSpend: AiStatus = {
      ...key,
      session: { analysisUsd: 0.03, chatUsd: 0, chatMessages: 0, picturesUsd: 0, pictures: 0 },
    };
    vi.mocked(aiBridge.status).mockResolvedValue(withSpend);
    creditMoved();
    await settle();
    expect(useAi.getState().status).toEqual(withSpend);
  });

  it('asks once at a time: jobs that end while it asks make one more ask after it, not one each', async () => {
    setAiStatus(relay);
    const answer = heldAnswer();
    creditMoved();
    creditMoved();
    creditMoved();
    expect(aiBridge.status).toHaveBeenCalledTimes(1);
    vi.mocked(aiBridge.status).mockResolvedValue(spent);
    answer(relay);
    await settle();
    expect(aiBridge.status).toHaveBeenCalledTimes(2);
    expect(useAi.getState().status).toEqual(spent);
    await settle();
    expect(aiBridge.status).toHaveBeenCalledTimes(2);
  });

  it('asks nothing with nothing to pay with', async () => {
    setAiStatus({ mode: 'none' });
    creditMoved();
    await settle();
    expect(aiBridge.status).not.toHaveBeenCalled();
  });
});
