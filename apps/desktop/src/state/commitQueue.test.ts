import { expect, it } from 'vitest';
import { afterCommits, beginCommit, commitsDone, onSettling } from './commitQueue';

it('an export’s drain waits for a nudge that a press waiting ahead of it left settling', async () => {
  const order: string[] = [];
  let started = false;
  let landNudge: () => void = () => undefined;
  // The keyboard's burst: what a waiting `.` started, sent when flushed and holding the queue until it lands.
  onSettling(() => {
    if (!started) return false;
    started = false;
    const end = beginCommit();
    order.push('nudge sent');
    landNudge = () => {
      order.push('nudge landed');
      end();
    };
    return true;
  });
  const landTrim = beginCommit();
  afterCommits(() => (started = true));
  const drained = commitsDone().then((open) => order.push(`drained ${open}`));
  landTrim();
  await Promise.resolve();
  expect(order).toEqual(['nudge sent']);
  landNudge();
  await drained;
  expect(order).toEqual(['nudge sent', 'nudge landed', 'drained true']);
});
