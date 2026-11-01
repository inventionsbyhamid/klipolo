import { useLingui } from '@lingui/react/macro';
import type { TextStyle } from '../../bindings/TextStyle';
import { faceCss, useFace } from '../../lib/fonts';
import { CAPTION_LOOKS, lookPatch, textLookCss, wearsLook, type TextLook, type TextLookPreset } from '../../lib/textLook';

/** `style`'s look drawn small on the sample backdrop, in the text's own font. */
function Sample({ style, text }: { style: TextStyle; text: string }) {
  useFace(style);
  return (
    <span aria-hidden className="text-look-sample">
      <span style={{ ...textLookCss(style, 15), ...faceCss(style), whiteSpace: 'nowrap' }}>{text}</span>
    </span>
  );
}

/**
 * The one-click looks (colour, highlight, outline and shadow together), each drawn in its own look on a sample
 * backdrop in the text's font, the one `style` wears pressed. `onPick` gets the look and the change it makes at this
 * text size.
 */
export function TextLooks({
  style,
  onPick,
}: {
  style: TextStyle;
  onPick: (look: TextLookPreset, patch: TextLook) => void;
}) {
  const { t } = useLingui();
  const sample = t({
    id: 'text.style.sample',
    message: 'Aa',
    comment: 'Two letters showing a text look; use letters of your script',
  });
  return (
    <div role="group" aria-label={t({ id: 'text.looks', message: 'Looks' })} className="text-looks">
      {CAPTION_LOOKS.map((look) => {
        const patch = lookPatch(look, style.size);
        return (
          <button
            key={look.key}
            type="button"
            tabIndex={0}
            className="text-look"
            aria-pressed={wearsLook(style, look)}
            onClick={() => onPick(look, patch)}
          >
            <Sample style={{ ...style, ...patch }} text={sample} />
            <span className="text-look-name">{t(look.name)}</span>
          </button>
        );
      })}
    </div>
  );
}
