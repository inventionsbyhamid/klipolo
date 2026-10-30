import { describe, expect, it } from 'vitest';
import { formatTime } from './time';

describe('formatTime', () => {
  it('shows tenths under an hour', () => {
    expect(formatTime(0)).toBe('0:00.0');
    expect(formatTime(4 * 60_000_000 + 24_650_000)).toBe('4:24.6');
    expect(formatTime(59 * 60_000_000 + 59_999_999)).toBe('59:59.9');
  });

  it('shows hours, minutes and seconds from an hour', () => {
    expect(formatTime(3_600_000_000)).toBe('1:00:00');
    expect(formatTime(3_600_000_000 + 65_000_000)).toBe('1:01:05');
  });

  it('never shows a negative time', () => {
    expect(formatTime(-5)).toBe('0:00.0');
  });
});
