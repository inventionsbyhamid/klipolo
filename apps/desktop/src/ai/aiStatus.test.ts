import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AiStatus } from '../bindings/AiStatus';

vi.mock('../bridge/ai', () => ({
  aiBridge: { status: vi.fn(), setKey: vi.fn(), setCode: vi.fn(), clear: vi.fn(), onStatus: vi.fn() },
}));
const { aiBridge } = await import('../bridge/ai');
const { followAiStatus, refreshAiStatus, setAiStatus, useAi } = await import('./aiStatus');

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
