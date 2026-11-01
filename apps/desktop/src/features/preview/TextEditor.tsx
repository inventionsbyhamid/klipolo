import { useLingui } from '@lingui/react/macro';
import { useEffect, useRef } from 'react';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Item } from '../../bindings/Item';
import type { TextAlign } from '../../bindings/TextAlign';
import { faceCss, useFace } from '../../lib/fonts';
import type { Size } from '../../lib/geometry';
import { resolveRect, textScale } from '../../lib/layout';
import { previewScale } from '../../lib/preview';
import { caretColor } from '../../lib/textLook';
import { useEngine } from '../../state/engine';
import { boxStyle } from './layerBox';
import { wrapWidth } from './textBox';
import { useTextEdit } from './textEdit';
import { useDevicePixelRatio } from './useDevicePixelRatio';
import { editKey } from './textEditKeys';
import { TextEditorLook } from './TextEditorLook';

interface TextEditorProps {
  /** The layer being typed (or the new box), at its rest look: never the motion's sampled box. */
  item: Item;
  /** False once the edit has finished: only its look stays, until the preview draws the layer again. */
  editable: boolean;
  canvas: Size;
  format: FormatKind;
  primary: FormatKind;
}

/**
 * Where the wrap width sits against the box (`shaping` in the engine's text layer): its slack goes after a left-aligned
 * box, before a right-aligned one and either side of a centred one, so the aligned edge is where the engine draws it.
 */
const JUSTIFY: Record<TextAlign, 'flex-start' | 'center' | 'flex-end'> = {
  left: 'flex-start',
  center: 'center',
  right: 'flex-end',
};

/** After Escape or Command Return, the layer's own box takes focus, so Return edits it again. */
function focusLayer(id: string) {
  const layer = [...document.querySelectorAll<HTMLElement>('[data-layer]')].find(
    (el) => el.dataset.layer === id,
  );
  layer?.focus({ preventScroll: true });
}

/**
 * Typing a title on the preview (a double click edits the text in place): a textarea over the canvas, in
 * the layer's face, size, colour, alignment and rotation, with its look drawn behind it (`TextEditorLook`). Native IME,
 * emoji and selection. The preview leaves the layer out meanwhile (`engine.setEditing`). It wraps at the engine's
 * width, its aligned edge where the engine puts it, and its lines are centred on the box as the engine centres them,
 * overflowing above and below alike. It is typed at full opacity.
 */
export function TextEditor({ item, editable, canvas, format, primary }: TextEditorProps) {
  const { t } = useLingui();
  const area = useRef<HTMLTextAreaElement>(null);
  const text = useTextEdit((s) => (s.session ?? s.closing)?.text ?? '');
  const content = item.content as Extract<Item['content'], { type: 'text' }>;
  const style = content.style;
  useFace(style);

  const rect = resolveRect(item.layout, primary, format);
  const px =
    (style.size * textScale(item.layout, primary, format) * Math.min(canvas.width, canvas.height)) / 1080;
  // The engine's wrap width: the box plus its slack, never wider (`minmax(0, 1fr)`: a long word breaks inside it). Its
  // 4 px are the preview's own pixels; with no engine drawing, CSS pixels.
  const maxPixels = useEngine((s) => (s.info && s.info.port !== 0 ? s.info.maxPreviewPixels : null));
  const dpr = useDevicePixelRatio();
  const unit = maxPixels === null ? 1 : previewScale(canvas, dpr, maxPixels);
  const width = wrapWidth(rect.w * canvas.width, unit);
  const draft = useTextEdit((s) => s.session?.draft != null);

  useEffect(() => {
    const el = area.current;
    if (!editable || !el) return;
    el.focus({ preventScroll: true });
    // A new box opens with "Your text" selected, so typing replaces it; a title's cursor goes to the end.
    if (draft) el.select();
    else el.setSelectionRange(el.value.length, el.value.length);
    // Once per edit: typing moves the cursor itself.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editable]);

  return (
    <div
      data-text-edit
      className={editable ? 'text-edit is-editing' : 'text-edit'}
      // Above every layer's box, the lifted ones (z 1) included: a press beside the words keeps typing.
      style={{
        ...boxStyle(rect, canvas, item.layout.rotation),
        justifyContent: JUSTIFY[style.align],
        zIndex: 2,
      }}
      onMouseDown={(e) => {
        // A press in the box beside the words keeps typing.
        if (e.target !== area.current) {
          e.preventDefault();
          area.current?.focus({ preventScroll: true });
        }
      }}
    >
      <div
        className="text-edit-cell"
        // Full opacity while typed; the finished edit's look is the layer's own until the preview draws it.
        style={{ width, gridTemplateColumns: 'minmax(0, 1fr)', opacity: editable ? 1 : item.layout.opacity }}
      >
        <TextEditorLook text={text} style={style} px={px} fill={!editable} />
        {editable && (
          <textarea
            ref={area}
            data-text-edit
            aria-label={t({
              id: 'textEdit.label',
              message: 'Text, editing',
              comment: 'Screen-reader name of the box for typing a title’s words on the video',
            })}
            rows={1}
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            autoComplete="off"
            value={text}
            onChange={(e) => useTextEdit.getState().setText(e.target.value)}
            onCompositionStart={() => useTextEdit.getState().setComposing(true)}
            onCompositionEnd={(e) => {
              useTextEdit.getState().setText(e.currentTarget.value);
              useTextEdit.getState().setComposing(false);
            }}
            onKeyDown={(e) => {
              const key = editKey({
                key: e.key,
                metaKey: e.metaKey,
                ctrlKey: e.ctrlKey,
                composing: e.nativeEvent.isComposing || e.keyCode === 229,
              });
              if (key === 'composing') return;
              // Space, J, K, L, Delete and the arrows type here, never reaching the editor's shortcuts.
              e.stopPropagation();
              if (key === 'finish') {
                e.preventDefault();
                void useTextEdit
                  .getState()
                  .finish()
                  .then(() => {
                    // A title begun while this one handed over (Add title, Text right after) keeps the focus.
                    if (!useTextEdit.getState().session) focusLayer(item.id);
                  });
              }
            }}
            onBlur={(e) => {
              // Tab, or focus moved elsewhere by the keyboard: the edit finishes. A press outside finishes it too
              // (`useEditEnds`); the window losing focus doesn't.
              const next = e.relatedTarget;
              if (next instanceof Element && !next.closest('[data-text-edit]'))
                void useTextEdit.getState().finish();
            }}
            className="text-edit-area"
            style={{
              ...faceCss(style),
              fontSize: px,
              lineHeight: 1.2,
              textAlign: style.align,
              color: style.color,
              caretColor: caretColor(style),
            }}
          />
        )}
      </div>
    </div>
  );
}
