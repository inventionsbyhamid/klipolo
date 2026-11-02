import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { FIRST_ROWS, ROWS_A_FRAME, useRowBudget } from './rowBudget';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

/** One frame's wait. */
const frame = () => act(() => vi.advanceTimersByTime(20));

describe('a long library tab', () => {
  it('draws its first rows at once and more each frame after, so no frame draws them all', () => {
    const total = FIRST_ROWS + 2 * ROWS_A_FRAME + 5;
    const { result, rerender } = renderHook(({ key }) => useRowBudget(key, total), {
      initialProps: { key: 'clips\n' },
    });
    expect(result.current).toBe(FIRST_ROWS);
    frame();
    expect(result.current).toBe(FIRST_ROWS + ROWS_A_FRAME);
    frame();
    frame();
    expect(result.current).toBe(Infinity);
    // Another tab or search starts again from the first rows.
    rerender({ key: 'music\n' });
    expect(result.current).toBe(FIRST_ROWS);
    for (let i = 0; i < 3; i++) frame();
    expect(result.current).toBe(Infinity);
    // Drawn again for anything else (an edit): every row at once.
    rerender({ key: 'music\n' });
    expect(result.current).toBe(Infinity);
  });

  it('a short tab draws every row at once', () => {
    const { result } = renderHook(() => useRowBudget('sounds\n', FIRST_ROWS));
    expect(result.current).toBe(Infinity);
  });
});
