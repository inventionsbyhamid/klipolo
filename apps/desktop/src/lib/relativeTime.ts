import { plural, t } from '@lingui/core/macro';

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;
/** From a week on, the day it was opened says more than a count of days. */
const DATE_FROM_MS = 7 * DAY_MS;

const DAY = new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric' });
const DAY_AND_YEAR = new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric', year: 'numeric' });

/**
 * When a project was last opened, as the start screen's card says it: "Opened just now" (under a minute), "Opened 5
 * minutes ago", "Opened 3 hours ago", "Opened yesterday", "Opened 3 days ago", then the day ("Opened Oct 12", with the
 * year when it isn't this year's). Whole minutes, hours and days, counted down. A time after `nowMs` (a clock that went
 * back) is just now.
 */
export function openedLabel(openedMs: number, nowMs: number): string {
  const agoMs = Math.max(0, nowMs - openedMs);
  if (agoMs < MINUTE_MS) return t({ id: 'start.opened.justNow', message: 'Opened just now' });
  if (agoMs < HOUR_MS) {
    const minutes = Math.floor(agoMs / MINUTE_MS);
    return t({
      id: 'start.opened.minutes',
      message: plural(minutes, { one: 'Opened # minute ago', other: 'Opened # minutes ago' }),
    });
  }
  if (agoMs < DAY_MS) {
    const hours = Math.floor(agoMs / HOUR_MS);
    return t({
      id: 'start.opened.hours',
      message: plural(hours, { one: 'Opened # hour ago', other: 'Opened # hours ago' }),
    });
  }
  if (agoMs < 2 * DAY_MS) return t({ id: 'start.opened.yesterday', message: 'Opened yesterday' });
  if (agoMs < DATE_FROM_MS) {
    const days = Math.floor(agoMs / DAY_MS);
    return t({ id: 'start.opened.days', message: `Opened ${days} days ago` });
  }
  const opened = new Date(openedMs);
  const day = (opened.getFullYear() === new Date(nowMs).getFullYear() ? DAY : DAY_AND_YEAR).format(opened);
  return t({
    id: 'start.opened.date',
    message: `Opened ${day}`,
    comment: '{day} is a short date, e.g. "Oct 12"',
  });
}
