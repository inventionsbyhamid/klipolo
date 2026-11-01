import { useLingui } from '@lingui/react/macro';
import { useEffect, useId, useRef } from 'react';
import { Button } from '../../design/Button';
import { useIntlLocale } from '../../i18n/react';
import { formatDownloadSize } from '../../lib/format';

/**
 * The one confirmation before anything is fetched from Google, in place where the person picked: the font menu. Never
 * a modal. Download is focused; Escape cancels. Once confirmed, Klipolo never asks again (Settings' switch stays).
 */
export function GoogleConsent({
  bytes,
  onDownload,
  onCancel,
}: {
  /** What the pick downloads. */
  bytes: number;
  onDownload: () => void;
  onCancel: () => void;
}) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const title = useId();
  const body = useId();
  const download = useRef<HTMLButtonElement>(null);
  useEffect(() => download.current?.focus(), []);
  const size = formatDownloadSize(locale, bytes);
  return (
    <section
      role="group"
      aria-labelledby={title}
      aria-describedby={body}
      className="font-consent"
      onKeyDown={(e) => {
        // A Return held down from the pick never repeats into the consent.
        if (e.repeat && (e.key === 'Enter' || e.key === ' ')) {
          e.preventDefault();
          return;
        }
        if (e.key !== 'Escape') return;
        // Escape goes back to where the pick was, not out of the menu.
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }}
    >
      <h3 id={title} className="font-consent-title">
        {t({ id: 'fontMenu.consent.title', message: 'Download from Google Fonts?' })}
      </h3>
      <p id={body} className="field-note">
        {t({
          id: 'fontMenu.consent.body',
          message:
            'Google Fonts are downloaded straight from Google, which sees your computer’s internet address and the fonts you preview and download. Downloaded fonts stay on this Mac and work offline.',
        })}
      </p>
      <div className="font-consent-actions">
        <Button ref={download} variant="primary" size="sm" onClick={onDownload}>
          {t({
            id: 'fontMenu.consent.download',
            message: `Download (${size})`,
            comment: 'Confirms downloading a Google font, and how much it downloads (“2.9 MB”)',
          })}
        </Button>
        <Button variant="ghost" size="sm" onClick={onCancel}>
          {t({ id: 'fontMenu.consent.cancel', message: 'Cancel' })}
        </Button>
      </div>
    </section>
  );
}
