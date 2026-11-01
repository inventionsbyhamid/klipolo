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
 * A quiet pill in the top bar while a clip's footage is being written backward (Reverse): "Reversing intro.mov" with
 * its progress ring, as the editing copies' pill shows theirs, and a button that stops it. One pill for the oldest copy
 * being made; nothing at all when none is.
 */
function Pill() {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const job = useReversing((s) => s.jobs[0]);
  if (!job) return null;
  const percent = Math.min(100, Math.max(0, Math.floor(job.progress * 100)));
  const name = reversingName(job.name);
  const stop = t({ id: 'timeline.reverse.stop', message: 'Stop reversing', comment: "The Reversing pill's stop button" });
  return (
    <span className="copies-pill" role="status" aria-label={`${name}, ${formatPercent(locale, percent / 100)}`}>
      <Ring percent={percent} />
      {name}
      <button type="button" className="icon-btn" aria-label={stop} title={stop} onClick={() => stopReversing(job.clipId)}>
        <Icon name="x" />
      </button>
    </span>
  );
}

export const ReversingPill = memo(Pill);
