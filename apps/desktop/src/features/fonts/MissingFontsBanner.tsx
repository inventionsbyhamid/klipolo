import { plural } from '@lingui/core/macro';
import { useLingui } from '@lingui/react/macro';
import { CircleAlert, X } from 'lucide-react';
import { useEffect } from 'react';
import { Button } from '../../design/Button';
import { IconButton } from '../../design/IconButton';
import { useIntlLocale } from '../../i18n/react';
import { formatDownloadSize } from '../../lib/format';
import { useFonts } from '../../state/fonts';
import { dismissMissing, openingKey, refreshMissing, startMissingFeed, useMissingFonts } from '../../state/missingFonts';
import { useProject } from '../../state/project';
import { DownloadLine } from './DownloadLine';
import { GoogleConsent } from './GoogleConsent';
import { ReplaceFonts, UseCurrent } from './ReplaceFonts';
import { downloadAll, downloadAllBytes } from './replaceOps';
import { cjkLanguageName } from './useCjkFonts';
import { useGooglePick } from './useGooglePick';
import './fonts.css';

/**
 * Over the preview when the open project uses fonts that aren't on this Mac, once per opening: how many, each Chinese,
 * Japanese or Korean text that needs its Noto Sans, "Download ({size})" when every one can be downloaded at the
 * project's own version (it downloads them in turn, after the one consent if it was never given), "Use {family}
 * {version}" for a version Google no longer serves, "Replace fonts…", and a way to dismiss it. The list is Rust's
 * (`fonts_missing`), asked again after each edit and when fonts change. Inter draws them meanwhile.
 */
export function MissingFontsBanner() {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const missing = useMissingFonts((s) => s.missing);
  const dismissed = useMissingFonts((s) => s.dismissed);
  const google = useFonts((s) => s.googleFonts);
  const opening = useProject((s) => openingKey(s.snapshot));
  const pick = useGooglePick();
  useEffect(() => {
    startMissingFeed();
    void refreshMissing();
  }, []);
  if (missing.length === 0 || (dismissed !== null && dismissed === opening)) return null;
  const count = missing.length;
  const bytes = downloadAllBytes(missing);
  const size = bytes === null ? null : formatDownloadSize(locale, bytes);
  const families = [...new Set(missing.map((m) => m.font.family))];
  return (
    <section className="missing-fonts" aria-label={t({ id: 'fontMenu.missing.label', message: 'Missing fonts' })}>
      <CircleAlert size={16} aria-hidden className="missing-fonts-icon" />
      <div className="missing-fonts-text" role="status">
        <p>
          {t({
            id: 'fontMenu.missing.banner',
            message: plural(count, {
              one: '# font used here isn’t on this Mac.',
              other: '# fonts used here aren’t on this Mac.',
            }),
            comment: 'Shown when a project opens that uses fonts this computer doesn’t have; Inter draws them meanwhile',
          })}
        </p>
        {missing
          .filter((m) => m.cjk !== null)
          .map((m) => {
            const language = m.cjk ? cjkLanguageName(m.cjk, locale) : '';
            const family = m.font.family;
            return (
              <p key={m.font.family} className="field-note">
                {t({
                  id: 'fontMenu.missing.cjk',
                  message: `${language} text needs ${family}.`,
                  comment: '{language} is a language (“Japanese”), {family} the Noto Sans font its text needs',
                })}
              </p>
            );
          })}
      </div>
      {pick.asking ? (
        <GoogleConsent bytes={pick.asking.bytes} onDownload={() => void pick.confirm()} onCancel={pick.cancelAsk} />
      ) : (
        <div className="missing-fonts-actions">
          {size !== null && google && (
            <Button
              size="sm"
              variant="primary"
              onClick={(e) => {
                const first = missing[0];
                void pick.pick({
                  family: first.font.family,
                  bytes: bytes ?? 0,
                  run: async () => {
                    await downloadAll(missing);
                    return first.font;
                  },
                  from: e.currentTarget,
                });
              }}
            >
              {t({
                id: 'fontMenu.missing.download',
                message: `Download (${size})`,
                comment: 'Downloads every font this project uses that isn’t on this Mac, and how much (“2.9 MB”)',
              })}
            </Button>
          )}
          {missing.map((m) => (
            <UseCurrent key={`${m.cjk ?? ''}${m.font.family}${JSON.stringify(m.font.source)}`} m={m} />
          ))}
          <ReplaceFonts missing={missing} />
          <IconButton
            icon={X}
            className="icon-btn-sm"
            label={t({ id: 'fontMenu.missing.dismiss', message: 'Dismiss', comment: 'Hides the missing-fonts notice' })}
            onClick={dismissMissing}
          />
        </div>
      )}
      {families.map((family) => (
        <DownloadLine key={family} family={family} />
      ))}
      {pick.error && (
        <p aria-live="polite" className="missing-fonts-error">
          {pick.error}
        </p>
      )}
    </section>
  );
}
