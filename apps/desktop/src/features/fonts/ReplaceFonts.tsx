import { useLingui } from '@lingui/react/macro';
import { useId, useState } from 'react';
import type { CjkScript } from '../../bindings/CjkScript';
import type { MissingFont } from '../../bindings/MissingFont';
import { Button } from '../../design/Button';
import { Popover, PopoverContent, PopoverTrigger } from '../../design/Popover';
import { useIntlLocale } from '../../i18n/react';
import { useProject } from '../../state/project';
import { FontMenu } from './FontMenu';
import { commitCurrent, commitReplace, replaceRows, type Choice } from './replaceOps';
import { cjkLanguageName } from './useCjkFonts';
import { useWeightName } from './WeightMenu';

/** A few characters of each script: a CJK row's "Covers my text" when only the captions use it. Never shown. */
const SAMPLE: Record<CjkScript, string> = { sc: '中文', tc: '中文', jp: '日本語', kr: '한국어' };

/** The words of the first text layer that uses `m`, for its menu's "Covers my text". */
function textOf(m: MissingFont): string {
  const tracks = useProject.getState().snapshot?.project.timeline.tracks ?? [];
  for (const item of tracks.flatMap((t) => t.items))
    if ((m.layers.includes(item.id) || m.hiddenLayers.includes(item.id)) && item.content.type === 'text')
      return item.content.text;
  return m.cjk ? SAMPLE[m.cjk] : '';
}

/**
 * "Replace fonts…": a popover with one row per missing family and weight, "{family} {weight}", each with the font menu,
 * and Replace: one undo step, "Replace fonts", for every row picked. A Chinese, Japanese or Korean row reads
 * "{language} text", and its menu opens with "Covers my text" on, so the person can pick a font on this Mac that has
 * those characters.
 */
export function ReplaceFonts({ missing, onReplaced }: { missing: MissingFont[]; onReplaced?: () => void }) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const nameOf = useWeightName();
  const title = useId();
  const [open, setOpen] = useState(false);
  const [picks, setPicks] = useState<Map<string, Choice>>(() => new Map());
  const rows = replaceRows(missing);
  const replace = () => {
    if (picks.size === 0) return;
    const chosen = picks;
    setOpen(false);
    setPicks(new Map());
    void commitReplace(missing, chosen).then((ok) => ok && onReplaced?.());
  };
  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setPicks(new Map());
      }}
    >
      <PopoverTrigger asChild>
        <Button size="sm">
          {t({
            id: 'fontMenu.replace',
            message: 'Replace fonts…',
            comment: 'Opens a list of the fonts that aren’t on this Mac, each with a menu to pick another',
          })}
        </Button>
      </PopoverTrigger>
      <PopoverContent className="replace-fonts" aria-labelledby={title}>
        <h3 id={title} className="font-consent-title">
          {t({ id: 'fontMenu.replace.title', message: 'Replace fonts' })}
        </h3>
        <ul className="replace-fonts-list">
          {rows.map((row) => {
            const m = row.missing;
            const choice = picks.get(row.key);
            const weight = nameOf(row.weight, row.italic);
            const family = m.font.family;
            const language = m.cjk ? cjkLanguageName(m.cjk, locale) : '';
            const label = m.cjk
              ? t({
                  id: 'fontMenu.replace.cjkRow',
                  message: `${language} text`,
                  comment: 'A row for Chinese, Japanese or Korean text whose font isn’t on this Mac (“Japanese text”)',
                })
              : // "{family} {weight}" ("Poppins Bold"): the font's name and its weight's, already in the person's language.
                `${family} ${weight}`;
            return (
              <li key={row.key} className="replace-fonts-row">
                <span className="replace-fonts-name" title={label}>
                  {label}
                </span>
                <FontMenu
                  label={t({ id: 'text.font', message: 'Font' })}
                  value={choice?.font ?? m.font}
                  weight={choice?.weight ?? row.weight}
                  italic={choice?.italic ?? row.italic}
                  text={m.cjk ? textOf(m) : ''}
                  onPick={(font, w, i) => setPicks((all) => new Map(all).set(row.key, { font, weight: w, italic: i }))}
                />
              </li>
            );
          })}
        </ul>
        <Button
          variant="primary"
          size="sm"
          className="replace-fonts-apply"
          aria-disabled={picks.size === 0 || undefined}
          onClick={replace}
        >
          {t({ id: 'fontMenu.replace.apply', message: 'Replace', comment: 'Replaces the fonts picked above' })}
        </Button>
      </PopoverContent>
    </Popover>
  );
}

/** "Use {family} {version}" for a Google font whose version Google no longer serves: a Replace, one undo step. */
export function UseCurrent({ m }: { m: MissingFont }) {
  const { t } = useLingui();
  const current = m.current;
  if (current?.source.type !== 'google') return null;
  const family = current.family;
  const version = current.source.version;
  return (
    <Button size="sm" onClick={() => void commitCurrent(m)}>
      {t({
        id: 'fontMenu.missing.useCurrent',
        message: `Use ${family} ${version}`,
        comment: 'Replaces a font’s old version, which Google no longer offers, with the one it offers now (“v22”)',
      })}
    </Button>
  );
}
