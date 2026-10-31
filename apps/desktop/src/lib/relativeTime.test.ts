import { describe, expect, it } from 'vitest';
import { openedLabel } from './relativeTime';

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
// Noon on Oct 31 2026 in the machine's time zone: dates are worded in local time.
const NOW = new Date(2026, 9, 31, 12, 0).getTime();

describe('openedLabel', () => {
  it('opened_label_words_each_range', () => {
    const cases: [number, string][] = [
      [NOW, 'Opened just now'],
      [NOW - 59_000, 'Opened just now'],
      // A clock that went back never says "in 2 minutes".
      [NOW + 2 * MINUTE, 'Opened just now'],
      [NOW - MINUTE, 'Opened 1 minute ago'],
      [NOW - 5 * MINUTE - 30_000, 'Opened 5 minutes ago'],
      [NOW - 59 * MINUTE, 'Opened 59 minutes ago'],
      [NOW - HOUR, 'Opened 1 hour ago'],
      [NOW - 3 * HOUR - 20 * MINUTE, 'Opened 3 hours ago'],
      [NOW - 23 * HOUR - 59 * MINUTE, 'Opened 23 hours ago'],
      // From a day on, by the calendar: any time yesterday, then whole dates back.
      [NOW - DAY, 'Opened yesterday'],
      [new Date(2026, 9, 30, 0, 5).getTime(), 'Opened yesterday'],
      [NOW - 47 * HOUR, 'Opened 2 days ago'],
      [NOW - 2 * DAY, 'Opened 2 days ago'],
      [NOW - 3 * DAY, 'Opened 3 days ago'],
      [new Date(2026, 9, 25, 0, 5).getTime(), 'Opened 6 days ago'],
      // Seven dates back or more: the date.
      [NOW - 6 * DAY - 23 * HOUR, 'Opened Oct 24'],
      [NOW - 7 * DAY, 'Opened Oct 24'],
      [new Date(2026, 9, 12, 9, 30).getTime(), 'Opened Oct 12'],
      [new Date(2026, 0, 3, 18, 0).getTime(), 'Opened Jan 3'],
      // Another year says which.
      [new Date(2025, 11, 30, 8, 0).getTime(), 'Opened Dec 30, 2025'],
    ];
    for (const [openedMs, label] of cases) expect(openedLabel(openedMs, NOW), label).toBe(label);
    for (const [openedMs] of cases) expect(openedLabel(openedMs, NOW)).not.toMatch(/[\u2013\u2014]/);

    // At 9 in the morning: last night is hours ago, the night before is two days ago (not "yesterday", 35 hours on).
    const morning = new Date(2026, 9, 31, 9, 0).getTime();
    expect(openedLabel(new Date(2026, 9, 30, 22, 0).getTime(), morning)).toBe('Opened 11 hours ago');
    expect(openedLabel(new Date(2026, 9, 30, 8, 0).getTime(), morning)).toBe('Opened yesterday');
    expect(openedLabel(new Date(2026, 9, 29, 22, 0).getTime(), morning)).toBe('Opened 2 days ago');
  });
});
