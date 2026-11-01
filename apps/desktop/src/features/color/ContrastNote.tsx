import { useLingui } from '@lingui/react/macro';
import { AlertTriangle } from 'lucide-react';
import type { JSX } from 'react';
import { contrastRatio, LOW_CONTRAST } from '../../design/color/colorMath';

/**
 * A colour setting's note when its colour is hard to read on what surrounds it: under 3:1 against any of `against`
 * (the title's highlight or outline). Always one status element, so a pick that makes the colour hard to read is
 * spoken as it lands; empty, and no height, otherwise.
 */
export function ContrastNote({ color, against }: { color: string | null; against: readonly string[] }): JSX.Element {
  const { t } = useLingui();
  const low = color !== null && against.some((c) => contrastRatio(color, c) < LOW_CONTRAST);
  return (
    <p role="status" className="field-note contrast-note">
      {low && (
        <span className="contrast-note-text">
          <AlertTriangle aria-hidden size={12} />
          <span>
            {t({
              id: 'color.contrast.low',
              message: 'Hard to read: too close to the color around it.',
              comment: 'Under a color setting whose color is too close to the one around it (a highlight, an outline)',
            })}
          </span>
        </span>
      )}
    </p>
  );
}
