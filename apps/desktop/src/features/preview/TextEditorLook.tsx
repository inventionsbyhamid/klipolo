import type { TextStyle } from '../../bindings/TextStyle';
import { faceCss } from '../../lib/fonts';
import { textLookCss } from '../../lib/textLook';

/** Holds a last empty line open (a zero-width space). */
const ZWSP = '​';

interface TextEditorLookProps {
  text: string;
  style: TextStyle;
  /** The font size in CSS pixels. */
  px: number;
  /** Draw the letters' fill too: when there is no textarea over it (the edit has finished). */
  fill: boolean;
}

/**
 * Behind the canvas editor's textarea, in the same grid cell, width and font: the same words with what a textarea
 * can't draw, the highlight box, outline and shadow, as the engine draws them (`textLookCss`). The textarea's letters
 * sit exactly on these.
 */
export function TextEditorLook({ text, style, px, fill }: TextEditorLookProps) {
  const font = faceCss(style);
  // A last empty line still takes its height, as it does in the textarea.
  const shown = text.endsWith('\n') ? `${text}${ZWSP}` : text;
  return (
    <div
      aria-hidden
      className="text-edit-look"
      style={{ ...font, fontSize: px, lineHeight: 1.2, textAlign: style.align }}
    >
      {shown && (
        // `anywhere`: the inline-block never grows past the column for a word wider than it, so it breaks that word on
        // the same lines as the textarea.
        <span
          style={{
            ...textLookCss(style, px),
            ...font,
            color: fill ? style.color : 'transparent',
            overflowWrap: 'anywhere',
          }}
        >
          {shown}
        </span>
      )}
    </div>
  );
}
