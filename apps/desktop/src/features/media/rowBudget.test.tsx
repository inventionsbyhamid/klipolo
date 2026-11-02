import { renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { FIRST_ROWS, useRowBudget } from './rowBudget';

describe('a long library tab', () => {
  it('draws its first rows at once and every row in the background render after, on each new tab or search', () => {
    const seen: number[] = [];
    const { rerender } = renderHook(({ key }) => seen.push(useRowBudget(key)), { initialProps: { key: 'clips\n' } });
    expect(seen).toEqual([FIRST_ROWS, Infinity]);
    seen.length = 0;
    rerender({ key: 'music\n' });
    expect(seen).toEqual([FIRST_ROWS, Infinity]);
    seen.length = 0;
    // Drawn again with the same tab (an edit): every row at once.
    rerender({ key: 'music\n' });
    expect(seen).toEqual([Infinity]);
  });
});
