import { act, renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { useMedia, useMediaVersion } from './media';

describe('media versions', () => {
  it('bumping one file changes only its version', () => {
    const screen = renderHook(() => useMediaVersion('/media/screen'));
    const mic = renderHook(() => useMediaVersion('/media/mic'));
    const none = renderHook(() => useMediaVersion(null));
    act(() => useMedia.getState().bump('/media/screen'));
    act(() => useMedia.getState().bump('/media/screen'));
    expect(screen.result.current).toBe(2);
    expect(mic.result.current).toBe(0);
    expect(none.result.current).toBe(0);
  });
});
