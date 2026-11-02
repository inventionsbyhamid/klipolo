import { useLingui } from '@lingui/react/macro';
import { memo } from 'react';
import { useIntlLocale } from '../../i18n/react';
import { Icon } from '../../layout/Icon';
import { formatPercent } from '../../lib/format';
import { useReversing } from '../../state/reversing';
import { reversingName, stopReversing } from '../timeline/commands';
import { Ring } from './MediaJobsPill';
import './mediaJobs.css';

/**
 * A quiet pill in the top bar while clips' footage is being written backward (Reverse): "Reversing intro.mov" with its
 * progress ring, as the editing copies' pill shows theirs, and a button that stops it. With more than one, "Reversing 2
 * clips", their progress together and one stop for all. Nothing at all when none is.
 */
function Pill() {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const jobs = useReversing((s) => s.jobs);
  if (jobs.length === 0) return null;
  const progress = jobs.reduce((sum, job) => sum + job.progress, 0) / jobs.length;
  const percent = Math.min(100, Math.max(0, Math.floor(progress * 100)));
  const count = jobs.length;
  const name =
    count === 1
      ? reversingName(jobs[0].name)
      : t({
          id: 'timeline.reverse.many',
          message: `Reversing ${count} clips`,
          comment: 'The Reversing pill while two or more clips are written backward; {count} is how many',
        });
  const stop = t({ id: 'timeline.reverse.stop', message: 'Stop reversing', comment: "The Reversing pill's stop button" });
  return (
    <span className="copies-pill" role="status" aria-label={`${name}, ${formatPercent(locale, percent / 100)}`}>
      <Ring percent={percent} />
      {name}
      <button
        type="button"
        // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
        tabIndex={0}
        className="icon-btn"
        aria-label={stop}
        title={stop}
        onClick={() => jobs.forEach((job) => stopReversing(job.clipId))}
      >
        <Icon name="x" />
      </button>
    </span>
  );
}

export const ReversingPill = memo(Pill);
