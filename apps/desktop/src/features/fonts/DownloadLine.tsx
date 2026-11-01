import { useLingui } from '@lingui/react/macro';
import { Button } from '../../design/Button';
import { useIntlLocale } from '../../i18n/react';
import { formatPercent } from '../../lib/format';
import { cancelDownload, useFonts } from '../../state/fonts';

/**
 * A Google download running now: "Downloading {family}… {percent}", its labelled bar and Cancel (its finished files
 * stay). Nothing while `family` isn't downloading. The font menu shows one per download under its list.
 */
export function DownloadLine({ family }: { family: string }) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const p = useFonts((s) => s.downloads[family]);
  if (!p) return null;
  const fraction = p.total > 0 ? Math.min(1, p.done / p.total) : 0;
  const percent = formatPercent(locale, fraction);
  return (
    <div className="font-download">
      <span className="font-download-text" title={family}>
        {t({
          id: 'fontMenu.downloadingFamily',
          message: `Downloading ${family}… ${percent}`,
          comment: '{family} is a Google font being downloaded; {percent} how far it has come',
        })}
      </span>
      <span
        role="progressbar"
        aria-label={t({
          id: 'fontMenu.downloadProgress',
          message: `Downloading ${family}`,
          comment: '{family} is a Google font being downloaded',
        })}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(fraction * 100)}
        className="font-bar"
      >
        <span style={{ width: `${fraction * 100}%` }} />
      </span>
      <Button
        size="sm"
        variant="ghost"
        aria-label={t({
          id: 'fontMenu.cancelDownload',
          message: `Cancel downloading ${family}`,
          comment: '{family} is a Google font being downloaded; its finished files stay',
        })}
        onClick={() => cancelDownload(family)}
      >
        {t({ id: 'fontMenu.cancel', message: 'Cancel' })}
      </Button>
    </div>
  );
}
