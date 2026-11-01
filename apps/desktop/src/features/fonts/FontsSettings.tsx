import { useLingui } from '@lingui/react/macro';
import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { CatalogueInfo } from '../../bindings/CatalogueInfo';
import type { CjkFonts } from '../../bindings/CjkFonts';
import type { DownloadedFamily } from '../../bindings/DownloadedFamily';
import { fonts } from '../../bridge/fonts';
import { Button } from '../../design/Button';
import { Menu, MenuContent, MenuRadioGroup, MenuRadioItem, MenuTrigger } from '../../design/Menu';
import { Toggle } from '../../design/Toggle';
import { useIntlLocale } from '../../i18n/react';
import { errorMessage } from '../../lib/errors';
import { formatDate, formatDownloadSize } from '../../lib/format';
import { Icon } from '../../layout/Icon';
import { downloadCjkFont, loadGoogleFonts, removeDownloaded, setGoogleFonts, useFonts } from '../../state/fonts';
import { toast } from '../../state/toasts';
import { DownloadLine } from './DownloadLine';
import { GoogleConsent } from './GoogleConsent';
import { cjkLanguageName, useCjkFonts } from './useCjkFonts';
import { useGooglePick } from './useGooglePick';
import { useWeightName } from './WeightMenu';
import './fonts.css';

/**
 * Settings' Fonts section: the "Google Fonts" switch (off: nothing is fetched from Google, and the font menu lists only
 * what's downloaded), the downloaded fonts with Remove, Show in Finder, the fonts for Chinese, Japanese and Korean, and
 * which Google Fonts list Klipolo ships ("Font list from {date}", or "Font list: sample (test build)").
 */
export function FontsSettings() {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const switchId = useId();
  const described = useId();
  const google = useFonts((s) => s.googleFonts);
  const [info, setInfo] = useState<CatalogueInfo | null>(null);
  useEffect(() => {
    let live = true;
    void loadGoogleFonts();
    fonts
      .catalogue()
      .then((answer) => live && setInfo(answer))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, []);
  // The list's day as written ("2026-10-22"), made a local calendar day, so no time zone shifts it.
  const [y, m, d] = (info?.date ?? '').split('-').map(Number);
  const date = y && m && d ? formatDate(locale, new Date(y, m - 1, d), { dateStyle: 'long' }) : '';
  return (
    <section className="fonts-settings" aria-labelledby="settings-fonts-title">
      <h3 className="section-title" id="settings-fonts-title">
        {t({ id: 'fontSettings.title', message: 'Fonts' })}
      </h3>
      <div className="fonts-settings-switch">
        <div className="fonts-settings-label">
          <label htmlFor={switchId}>{t({ id: 'fontSettings.google', message: 'Google Fonts' })}</label>
          <p id={described} className="hint">
            {t({ id: 'fontSettings.google.line', message: 'Search and download free fonts from Google.' })}
          </p>
        </div>
        <Toggle
          id={switchId}
          aria-describedby={described}
          checked={google}
          onCheckedChange={(on) => void setGoogleFonts(on).catch((e) => toast.error(errorMessage(e)))}
        />
      </div>
      <DownloadedFonts />
      <Button size="sm" className="fonts-settings-reveal" onClick={() => void fonts.reveal().catch(() => undefined)}>
        {t({ id: 'fontSettings.reveal', message: 'Show in Finder', comment: 'Shows the downloaded fonts’ folder' })}
      </Button>
      <CjkFontsSettings />
      {info && (
        <p className="hint">
          {info.sample || !date
            ? t({
                id: 'fontSettings.listSample',
                message: 'Font list: sample (test build)',
                comment: 'The list of Google fonts is a small sample, in a build made for testing',
              })
            : t({
                id: 'fontSettings.listDate',
                message: `Font list from ${date}`,
                comment: '{date} is the day the list of Google fonts was made',
              })}
        </p>
      )}
    </section>
  );
}

/** Where the keyboard goes once the in-place question closes: a row's Remove (by key), or the heading. */
const HEADING = '';

/**
 * "Downloaded fonts": each Google family downloaded on this Mac with its size and Remove, which asks in place
 * ("Projects that use {family} will show it as missing."). Removal names the family's id, never a path. Downloaded
 * fonts are never deleted by themselves. Asked again when the fonts change. A family downloaded at two versions shows
 * each one's version. The question focuses its Cancel; after it the keyboard goes back to the row's Remove, and after
 * a removal to the next row's Remove, else the heading.
 */
export function DownloadedFonts() {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const title = useId();
  const [list, setList] = useState<DownloadedFamily[] | null>(null);
  const [asking, setAsking] = useState<string | null>(null);
  const [focusTo, setFocusTo] = useState<string | null>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const removes = useRef(new Map<string, HTMLButtonElement>());
  const load = useCallback(() => {
    fonts
      .downloaded()
      .then(setList)
      .catch(() => setList([]));
  }, []);
  useEffect(() => {
    load();
    let stop: (() => void) | null = null;
    let live = true;
    listen('fonts-changed', load)
      .then((unlisten) => (live ? (stop = unlisten) : unlisten()))
      .catch(() => undefined);
    return () => {
      live = false;
      stop?.();
    };
  }, [load]);
  // The in-place question takes the keyboard to its Cancel, never to the Remove a second Return would press.
  useEffect(() => {
    if (asking) cancel.current?.focus();
  }, [asking]);
  useEffect(() => {
    if (focusTo === null) return;
    const to = focusTo === HEADING ? heading.current : removes.current.get(focusTo);
    if (!to) return;
    to.focus();
    setFocusTo(null);
  }, [focusTo, asking, list]);
  if (!list) return null;
  const key = (f: DownloadedFamily) => `${f.id.family}|${f.id.version}`;
  const twice = new Set(list.map((f) => f.id.family).filter((family, i, all) => all.indexOf(family) !== i));
  return (
    <section aria-labelledby={title} className="fonts-downloaded">
      <h4 id={title} ref={heading} tabIndex={-1} className="fonts-downloaded-title">
        {t({ id: 'fontSettings.downloaded', message: 'Downloaded fonts' })}
      </h4>
      {list.length === 0 && (
        <p className="hint">{t({ id: 'fontSettings.downloaded.none', message: 'No Google fonts downloaded yet.' })}</p>
      )}
      <ul className="fonts-downloaded-list">
        {list.map((f, at) => {
          const size = formatDownloadSize(locale, f.bytes);
          const name = f.id.family;
          const version = f.id.version;
          const family = twice.has(name)
            ? t({
                id: 'fontSettings.downloaded.version',
                message: `${name} (${version})`,
                comment: 'A Google font downloaded at two versions: its name and this one’s version (“Poppins (v23)”)',
              })
            : name;
          const k = key(f);
          const next = list[at + 1] ?? list[at - 1];
          return (
            <li key={k}>
              <div className="fonts-downloaded-row">
                <span className="fonts-downloaded-name" title={family}>
                  {family}
                </span>
                <span className="fonts-downloaded-size">{size}</span>
                {asking !== k && (
                  <Button
                    ref={(el) => {
                      if (el) removes.current.set(k, el);
                      else removes.current.delete(k);
                    }}
                    size="sm"
                    variant="ghost"
                    aria-label={t({
                      id: 'fontSettings.remove.label',
                      message: `Remove ${family}`,
                      comment: '{family} is a downloaded Google font',
                    })}
                    onClick={() => setAsking(k)}
                  >
                    {t({ id: 'fontSettings.remove', message: 'Remove' })}
                  </Button>
                )}
              </div>
              {asking === k && (
                <div
                  role="group"
                  className="fonts-downloaded-ask"
                  aria-label={t({
                    id: 'fontSettings.remove.ask',
                    message: `Remove ${family}?`,
                    comment: '{family} is a downloaded Google font; asked in place before it is removed',
                  })}
                >
                  <p className="hint">
                    {t({
                      id: 'fontSettings.remove.line',
                      message: `Projects that use ${family} will show it as missing.`,
                      comment: '{family} is a downloaded Google font about to be removed from this Mac',
                    })}
                  </p>
                  <div className="font-consent-actions">
                    <Button
                      size="sm"
                      onClick={() => {
                        setAsking(null);
                        // The row's own Remove until it has gone, then the next one's (or the heading).
                        setFocusTo(k);
                        removeDownloaded(f.id)
                          .then(() => {
                            setFocusTo(next ? key(next) : HEADING);
                            load();
                          })
                          .catch((e) => toast.error(errorMessage(e)));
                      }}
                    >
                      {t({ id: 'fontSettings.remove', message: 'Remove' })}
                    </Button>
                    <Button
                      ref={cancel}
                      size="sm"
                      variant="ghost"
                      onClick={() => {
                        setAsking(null);
                        setFocusTo(k);
                      }}
                    >
                      {t({ id: 'fontSettings.remove.cancel', message: 'Cancel' })}
                    </Button>
                  </div>
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/**
 * "Fonts for Chinese, Japanese and Korean": Noto Sans SC, TC, JP and KR, the only faces that text falls back to. Each
 * downloads a weight at a time: its weights with their sizes, and Download, after the one consent (in place), with its
 * progress and Cancel. While Google Fonts is off, nothing downloads. Each row is two lines, the language and its family
 * above what to download, so it fits in any language.
 */
export function CjkFontsSettings() {
  const { t } = useLingui();
  const title = useId();
  const list = useCjkFonts();
  const google = useFonts((s) => s.googleFonts);
  const pick = useGooglePick();
  if (!list) return null;
  return (
    <section aria-labelledby={title} className="cjk-fonts">
      <h4 id={title} className="fonts-downloaded-title">
        {t({ id: 'fontSettings.cjk', message: 'Fonts for Chinese, Japanese and Korean' })}
      </h4>
      {pick.asking && (
        <GoogleConsent bytes={pick.asking.bytes} onDownload={() => void pick.confirm()} onCancel={pick.cancelAsk} />
      )}
      {/* Hidden, not gone, while the consent asks: the keyboard goes back to the row's Download after it. */}
      <div hidden={!!pick.asking} className="cjk-fonts">
        {list.map((c) => (
          <CjkRow key={c.script} fonts={c} google={google} onDownload={pick.pick} />
        ))}
      </div>
      <p aria-live="polite" className="font-note font-error">
        {pick.error}
      </p>
    </section>
  );
}

function CjkRow({
  fonts: c,
  google,
  onDownload,
}: {
  fonts: CjkFonts;
  google: boolean;
  onDownload: ReturnType<typeof useGooglePick>['pick'];
}) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const name = useWeightName();
  const missing = c.faces.filter((f) => !f.downloaded);
  const here = c.faces.filter((f) => f.downloaded).map((f) => name(f.weight, false));
  const [weight, setWeight] = useState(() => missing.find((f) => f.weight === 400)?.weight ?? missing[0]?.weight);
  const busy = useFonts((s) => !!s.downloads[c.family]);
  const chosen = missing.find((f) => f.weight === weight) ?? missing[0];
  const names = here.join(', ');
  const family = c.family;
  const language = cjkLanguageName(c.script, locale);
  const faceText = (f: { weight: number; bytes: number }) => {
    const w = name(f.weight, false);
    const size = formatDownloadSize(locale, f.bytes);
    return t({
      id: 'fontSettings.cjk.face',
      message: `${w} (${size})`,
      comment: 'A weight of a Noto Sans and what it downloads (“Regular (5.4 MB)”)',
    });
  };
  return (
    <div className="cjk-row">
      <p className="cjk-row-name" title={`${language} · ${family}`}>
        {t({
          id: 'fontSettings.cjk.row',
          message: `${language} · ${family}`,
          comment: '{language} is a language (“Japanese”), {family} the Noto Sans it uses',
        })}
      </p>
      {chosen && (
        <div className="cjk-row-pick">
          <Menu>
            <MenuTrigger asChild>
              <button
                type="button"
                className="select"
                aria-label={t({
                  id: 'fontSettings.cjk.weight',
                  message: `${family} weight: ${faceText(chosen)}`,
                  comment: '{family} is a Noto Sans; then the weight picked and its size',
                })}
              >
                <span className="select-text">{faceText(chosen)}</span>
                <Icon name="chev-down" />
              </button>
            </MenuTrigger>
            <MenuContent>
              <MenuRadioGroup value={String(chosen.weight)} onValueChange={(v) => setWeight(Number(v))}>
                {missing.map((f) => (
                  <MenuRadioItem key={f.weight} value={String(f.weight)}>
                    {faceText(f)}
                  </MenuRadioItem>
                ))}
              </MenuRadioGroup>
            </MenuContent>
          </Menu>
          <Button
            size="sm"
            aria-disabled={!google || busy || undefined}
            aria-label={t({
              id: 'fontSettings.cjk.download',
              message: `Download ${family}`,
              comment: '{family} is a Noto Sans, for Chinese, Japanese or Korean text',
            })}
            onClick={(e) => {
              if (!google || busy) return;
              void onDownload({
                family,
                bytes: chosen.bytes,
                run: () => downloadCjkFont(c.script, family, chosen.weight),
                from: e.currentTarget,
              });
            }}
          >
            {t({ id: 'fontSettings.cjk.downloadShort', message: 'Download' })}
          </Button>
        </div>
      )}
      <DownloadLine family={family} />
      {here.length > 0 && (
        <p className="hint">
          {t({
            id: 'fontSettings.cjk.here',
            message: `Downloaded: ${names}`,
            comment: 'The weights of a Noto Sans on this Mac (“Regular, Bold”)',
          })}
        </p>
      )}
    </div>
  );
}
