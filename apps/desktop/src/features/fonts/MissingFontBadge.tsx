import { useLingui } from '@lingui/react/macro';
import { CircleAlert } from 'lucide-react';
import type { FontRef } from '../../bindings/FontRef';
import { sameFont } from '../../state/fonts';
import { useFontRowAnswer } from './useFontSearch';

/**
 * Whether `font` (its family and source, a Google font at its version) is on this Mac: a bundled one always is; else
 * Rust's row for it says (downloaded or installed), asked again when the fonts change. Null until Rust has answered.
 */
function useFontHere(font: FontRef, weight: number, italic: boolean): boolean | null {
  const answer = useFontRowAnswer(font, weight, italic);
  if (font.source.type === 'bundled') return true;
  if (answer === undefined) return null;
  return answer !== null && sameFont({ family: answer.family, source: answer.source }, font) && answer.state !== 'download';
}

/**
 * "Missing font: {family}" under a text's font menu while that very font isn't on this Mac: the preview draws Inter
 * meanwhile, never silently. Picking it in the font menu downloads a Google font again.
 */
export function MissingFontBadge({ font, weight, italic }: { font: FontRef; weight: number; italic: boolean }) {
  const { t } = useLingui();
  const here = useFontHere(font, weight, italic);
  if (here !== false) return null;
  const family = font.family;
  return (
    <p className="field-note font-missing">
      <CircleAlert size={12} aria-hidden />
      <span title={family}>
        {t({
          id: 'fontMenu.missing.badge',
          message: `Missing font: ${family}`,
          comment: '{family} is a font this text uses that isn’t on this Mac; Inter draws it meanwhile',
        })}
      </span>
    </p>
  );
}
