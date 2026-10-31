import { plural, t } from '@lingui/core/macro';

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;
/** From a week of days on, the day it was opened says more than a count of days. */
const DATE_FROM_DAYS = 7;

const DAY = new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric' });
const DAY_AND_YEAR = new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric', year: 'numeric' });

/** The local midnight starting the day `ms` falls on. */
function midnight(ms: number): number {
  const day = new Date(ms);
  return new Date(day.getFullYear(), day.getMonth(), day.getDate()).getTime();
}

/**
 * Calendar days from `fromMs` to `toMs` in local time: 1 for any time yesterday. Rounded, so a day of 23 or 25 hours
 * (a clock change) still counts as one.
 */
function daysBetween(fromMs: number, toMs: number): number {
  return Math.round((midnight(toMs) - midnight(fromMs)) / DAY_MS);
}

function hoursLabel(agoMs: number): string {
  const hours = Math.floor(agoMs / HOUR_MS);
  return t({
    id: 'start.opened.hours',
    message: plural(hours, { one: 'Opened # hour ago', other: 'Opened # hours ago' }),
  });
}

/**
 * When a project was last opened, as the start screen's card says it: "Opened just now" (under a minute), "Opened 5
 * minutes ago", "Opened 3 hours ago" (whole minutes and hours, counted down, under a day), then by the calendar: "Opened
 * yesterday", "Opened 3 days ago", then the day ("Opened Oct 12", with the year when it isn't this year's). A time after
 * `nowMs` (a clock that went back) is just now.
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
  if (agoMs < DAY_MS) return hoursLabel(agoMs);
  const days = daysBetween(openedMs, nowMs);
  // A full day ago and still the same date: the day the clocks went back was 25 hours long.
  if (days < 1) return hoursLabel(agoMs);
  if (days === 1) return t({ id: 'start.opened.yesterday', message: 'Opened yesterday' });
  if (days < DATE_FROM_DAYS) return t({ id: 'start.opened.days', message: `Opened ${days} days ago` });
  const opened = new Date(openedMs);
  const day = (opened.getFullYear() === new Date(nowMs).getFullYear() ? DAY : DAY_AND_YEAR).format(opened);
  return t({
    id: 'start.opened.date',
    message: `Opened ${day}`,
    comment: '{day} is a short date, e.g. "Oct 12"',
  });
}
