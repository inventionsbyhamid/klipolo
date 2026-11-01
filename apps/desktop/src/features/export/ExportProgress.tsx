import { useLingui } from '@lingui/react/macro';
import { useShallow } from 'zustand/react/shallow';
import { useIntlLocale } from '../../i18n/react';
import { formatNumber, formatPercent } from '../../lib/format';
import { FORMAT_SHORT } from '../../lib/layout';
import { exportPercent, NO_PAUSE_FINISHING, useExport } from '../../state/export';
import { timeLeft, videos } from './presets';

/**
 * While it runs: what it makes, the big percent and one bar for every format's frames, then the
 * frame it is at in each video and the time left. The formats render together in one pass, so a video's frame is the
 * run's frames shared out between them. Until the run has a job (its latest edits saved, its job written) it says so.
 */
export function ExportProgress() {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const s = useExport(
    useShallow((s) => ({
      files: s.files,
      formats: s.formats,
      jobId: s.jobId,
      frame: s.frame,
      totalFrames: s.totalFrames,
      etaS: s.etaS,
      finishing: s.finishing,
      retrying: s.retrying,
      retries: s.retries,
      pausing: s.pausing,
      stopping: s.stopping,
      saving: s.saving,
    })),
  );
  const percent = useExport(exportPercent);
  const formats = s.files.length > 0 ? s.files.map((f) => f.format) : s.formats;
  const count = Math.max(1, formats.length);
  const title =
    formats.length === 1
      ? t({ id: 'export.progress.one', message: `Exporting the ${FORMAT_SHORT[formats[0]]} video` })
      : t({ id: 'export.progress.many', message: `Exporting ${videos(formats.length)}` });
  const starting = !s.jobId;
  const where = starting
    ? s.stopping
      ? t({ id: 'export.progress.stopping', message: 'Stopping' })
      : s.saving
        ? t({ id: 'export.progress.saving', message: 'Saving your latest changes' })
        : t({ id: 'export.progress.starting', message: 'Starting' })
    : t({
        id: 'export.progress.frame',
        message: `Frame ${formatNumber(locale, Math.floor(s.frame / count))} of ${formatNumber(locale, Math.floor(s.totalFrames / count))}.`,
      });
  const left = starting
    ? ''
    : s.stopping
      ? t({ id: 'export.progress.stopping', message: 'Stopping' })
      : s.pausing
        ? t({ id: 'export.progress.pausing', message: 'Pausing' })
        : s.finishing
          ? t({ id: 'export.progress.finishing', message: 'Finishing' })
          : s.retrying
            ? t({ id: 'export.progress.retrying', message: 'Trying again' })
            : s.etaS !== null
              ? timeLeft(s.etaS)
              : t({ id: 'export.progress.estimating', message: 'Working out the time left' });
  return (
    <div className="ex-progress">
      <div className="ex-row">
        <span className="ex-title">{title}</span>
        <span className="ex-pct">{formatPercent(locale, percent / 100)}</span>
      </div>
      <div
        className="ex-bar"
        role="progressbar"
        aria-label={t({ id: 'export.progress.bar', message: 'Export progress' })}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <i style={{ width: `${percent}%` }} />
      </div>
      <div className="ex-row">
        <span className="ex-meta">{where}</span>
        {left && <span className="ex-meta">{left}</span>}
      </div>
      {/* Joining the parts can't pause: Pause says why (`aria-describedby`). */}
      {s.finishing && !starting && (
        <p className="ex-meta" id="export-no-pause">
          {NO_PAUSE_FINISHING}
        </p>
      )}
      {s.retries >= 2 && (
        <p className="ex-meta">
          {t({
            id: 'export.progress.compatibility',
            message: 'Trying again in compatibility mode. This is slower.',
          })}
        </p>
      )}
    </div>
  );
}
