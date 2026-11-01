import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushBursts } from '../../state/bursts';
import { useKeySettle } from './useKeySettle';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const settle = (commit = vi.fn<(color: string) => void>()) => ({
  commit,
  ...renderHook(({ fn }) => useKeySettle(fn), { initialProps: { fn: commit } }),
});

describe('useKeySettle', () => {
  it('commits the newest colour once, 400 ms after the last key', () => {
    const { commit, result } = settle();
    act(() => result.current.keyed('#111111'));
    act(() => vi.advanceTimersByTime(300));
    act(() => result.current.keyed('#222222'));
    act(() => vi.advanceTimersByTime(399));
    expect(commit).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(1));
    expect(commit).toHaveBeenCalledExactlyOnceWith('#222222');
    act(() => vi.advanceTimersByTime(1000));
    expect(commit).toHaveBeenCalledTimes(1);
  });

  it('flush, an undo (flushBursts) and unmount commit at once; with nothing pending they commit nothing', () => {
    const { commit, result, unmount } = settle();
    act(() => result.current.flush());
    expect(flushBursts()).toBe(false);
    act(() => result.current.keyed('#111111'));
    act(() => result.current.flush());
    act(() => result.current.keyed('#222222'));
    expect(flushBursts()).toBe(true);
    act(() => result.current.keyed('#333333'));
    unmount();
    expect(commit.mock.calls).toEqual([['#111111'], ['#222222'], ['#333333']]);
    act(() => vi.advanceTimersByTime(1000));
    expect(commit).toHaveBeenCalledTimes(3);
  });

  it('drop forgets the pending colour: nothing commits, then or later', () => {
    const { commit, result, unmount } = settle();
    act(() => result.current.keyed('#111111'));
    act(() => result.current.drop());
    expect(flushBursts()).toBe(false);
    act(() => vi.advanceTimersByTime(1000));
    unmount();
    expect(commit).not.toHaveBeenCalled();
  });

  it('commits through the newest commit after a re-render', () => {
    const { commit, result, rerender } = settle();
    const newer = vi.fn<(color: string) => void>();
    act(() => result.current.keyed('#111111'));
    rerender({ fn: newer });
    act(() => vi.advanceTimersByTime(400));
    expect(newer).toHaveBeenCalledExactlyOnceWith('#111111');
    expect(commit).not.toHaveBeenCalled();
  });
});
