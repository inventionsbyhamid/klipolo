import { useLingui } from '@lingui/react/macro';
import { Bold, TextAlignCenter, TextAlignEnd, TextAlignStart } from 'lucide-react';
import { useId } from 'react';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Item } from '../../bindings/Item';
import type { ItemContent } from '../../bindings/ItemContent';
import type { TextAlign } from '../../bindings/TextAlign';
import type { TextStyle } from '../../bindings/TextStyle';
import { IconButton } from '../../design/IconButton';
import { Segmented } from '../../design/Segmented';
import { Toggle } from '../../design/Toggle';
import type { Label } from '../../lib/labels';
import { BOX_COLOR, OUTLINE_STEPS, outlineStep, outlineWidth, scaledStyle, type TextLook } from '../../lib/textLook';
import { TEXT_SWATCHES } from '../../lib/swatches';
import { itemOp } from '../../lib/ops';
import { commitToggle } from '../../state/commit';
import { useRecentFonts } from '../../state/fonts';
import { commitLatest, contentNow } from '../../state/latest';
import { clearPreview, dropPendingPreview, previewOps } from '../../state/livePreview';
import { ColorField, type Swatch } from '../color/ColorField';
import { useColorCommit } from '../color/useColorCommit';
import { FontMenu } from '../fonts/FontMenu';
import { AnimationField, PositionField, TimingField } from './common';
import { Field } from './fields';
import { TextField } from './TextField';
import { TextLooks } from './TextLooks';
import { ToolSlider } from './ToolSlider';
import './textInspector.css';

type TextContent = Extract<ItemContent, { type: 'text' }>;

/** The size slider's range: up to the largest size the model takes (`model.text.size`, 4 to 400); under 8 none reads. */
export const MIN_TEXT_SIZE = 8;
export const MAX_TEXT_SIZE = 400;

/** Bold's weight, and the weight it goes back to. */
const BOLD = 700;
const REGULAR = 400;


/** The text colours offered, then "+" for any other. */
export function textSwatches(t: (d: { id: string; message?: string }) => string): Swatch[] {
  return TEXT_SWATCHES.map((s) => ({ value: s.value, name: t(s.name) }));
}

/** The highlight's colours: the text colours, its navy the see-through box behind the words. */
function highlightSwatches(t: (d: { id: string; message?: string }) => string): Swatch[] {
  return textSwatches(t).map((s) => (s.value === '#121A33' ? { ...s, value: BOX_COLOR } : s));
}

/**
 * A text layer's fields: its words, Font, Size, Style (Bold and alignment), the one-click Looks, Color,
 * Highlight, Outline (with its width), Shadow, Position, Animation and Timing. Each change is one undo step, built at
 * its turn from the title as the project has it then (`contentNow`), so a change on its way (words just finished on
 * the canvas) is kept. The colours dry-run on the video while hovered; the text color says when it is hard to read on
 * its own highlight or outline.
 */
export function TextInspector({
  item,
  content,
  format,
  primary,
  locked,
}: {
  item: Item;
  content: TextContent;
  format: FormatKind;
  primary: FormatKind;
  locked: boolean;
}) {
  const { t } = useLingui();
  const style = content.style;
  const shadowId = useId();
  const styleOps = (change: (now: TextStyle) => TextStyle) => {
    const now = contentNow(item.id, content);
    return [itemOp(item, { content: { ...now, style: change(now.style) } })];
  };
  /** The style now with `patch` over it. */
  const lookOps = (patch: Partial<TextLook>) => styleOps((now) => ({ ...now, ...patch }));
  const setStyle = (label: Label, change: (now: TextStyle) => TextStyle) =>
    void commitLatest(label, () => styleOps(change));
  const sizeLabel = t({ id: 'text.size', message: 'Size' });
  const colorLabel = t({ id: 'text.color', message: 'Text color' });
  const highlightLabel = t({ id: 'text.highlight', message: 'Highlight', comment: 'A color band behind text' });
  const outlineLabel = t({ id: 'text.outline', message: 'Outline', comment: 'A line around the letters' });
  const shadowLabel = t({ id: 'text.shadow', message: 'Shadow' });

  const color = useColorCommit({
    value: style.color,
    label: 'text.color.undo',
    ops: (c) => (c === null ? [] : lookOps({ color: c })),
  });
  const highlight = useColorCommit({
    value: style.background,
    label: (c) =>
      c === null
        ? 'text.highlight.remove.undo'
        : style.background === null
          ? 'text.highlight.add.undo'
          : 'text.highlight.color.undo',
    ops: (background) => lookOps({ background }),
  });
  /** The outline `c` makes: its width kept, or medium for a new one. */
  const outlineOf = (c: string | null, now: TextStyle) =>
    c === null ? null : { color: c, width: now.outline?.width ?? outlineWidth(now.size, 'medium') };
  const outline = useColorCommit({
    value: style.outline?.color ?? null,
    label: (c) =>
      c === null ? 'text.outline.remove.undo' : style.outline === null ? 'text.outline.add.undo' : 'text.outline.color.undo',
    ops: (c) => styleOps((now) => ({ ...now, outline: outlineOf(c, now) })),
  });
  const step = outlineStep(style);
  const bold = style.weight >= 600;
  const aligns: { value: TextAlign; label: string; icon: typeof TextAlignStart }[] = [
    { value: 'left', label: t({ id: 'text.align.left', message: 'Align left' }), icon: TextAlignStart },
    { value: 'center', label: t({ id: 'text.align.center', message: 'Align center' }), icon: TextAlignCenter },
    { value: 'right', label: t({ id: 'text.align.right', message: 'Align right' }), icon: TextAlignEnd },
  ];

  return (
    <>
      <TextField key={content.text} id={item.id} content={content} locked={locked} />
      <Field label={t({ id: 'text.font', message: 'Font' })}>
        <FontMenu
          label={t({ id: 'text.font', message: 'Font' })}
          value={style.font}
          weight={style.weight}
          italic={style.italic}
          text={content.text}
          // Arrow keys and a resting hover show a font on the title (a dry run); Escape puts it back.
          onPreview={(font, weight, italic) =>
            font ? previewOps(() => styleOps((now) => ({ ...now, font, weight, italic }))) : clearPreview()
          }
          onPick={(font, weight, italic) => {
            // The pick is committed as the dry run showed it: the new revision replaces the dry run, no flicker.
            dropPendingPreview();
            useRecentFonts.getState().push(font);
            setStyle('text.font.undo', (now) => ({ ...now, font, weight, italic }));
          }}
        />
      </Field>
      <Field label={sizeLabel}>
        <ToolSlider
          label={sizeLabel}
          commitLabel="text.size.undo"
          value={style.size}
          min={MIN_TEXT_SIZE}
          max={MAX_TEXT_SIZE}
          step={1}
          format={(v) => String(Math.round(v))}
          ops={(size) => styleOps((now) => scaledStyle(now, size))}
        />
      </Field>
      <Field label={t({ id: 'text.style.field', message: 'Style', comment: 'Bold and alignment of a text' })}>
        <div className="text-style-row">
          <IconButton
            icon={Bold}
            className="icon-btn-sm"
            label={t({ id: 'text.bold', message: 'Bold' })}
            active={bold}
            // A switch decided at its turn, from the title's weight then: two quick presses turn it off and back on.
            onClick={() =>
              void commitToggle(
                () => contentNow(item.id, content).style.weight >= 600,
                (on) => (on ? 'text.bold.undo' : 'text.regular.undo'),
                (on) => styleOps((now) => ({ ...now, weight: on ? BOLD : REGULAR })),
                { id: item.id },
              )
            }
          />
          <Segmented
            label={t({ id: 'text.align', message: 'Text alignment' })}
            value={style.align}
            options={aligns}
            onChange={(align) => setStyle('text.align.undo', (now) => ({ ...now, align }))}
          />
        </div>
      </Field>
      <Field label={t({ id: 'text.looks', message: 'Looks' })} stack>
        <TextLooks
          style={style}
          onPick={(look, patch) =>
            void commitLatest(
              // The look by its own message id, worded in the language shown (`labels.ts`).
              { id: 'text.look.apply.undo', params: { style: { code: look.name.id, params: {} } } },
              () => lookOps(patch),
            )
          }
        />
      </Field>
      <Field label={t({ id: 'text.color.short', message: 'Color', comment: 'The text color field' })}>
        <ColorField
          label={colorLabel}
          presets={textSwatches(t)}
          // Hard to read on its own highlight or outline: a note says so.
          contrastWith={[style.background, style.outline?.color ?? null].filter((c): c is string => c !== null)}
          {...color}
        />
      </Field>
      <Field label={highlightLabel}>
        <ColorField
          label={highlightLabel}
          presets={highlightSwatches(t)}
          none={{ label: t({ id: 'text.highlight.none', message: 'No highlight' }) }}
          alpha
          {...highlight}
        />
      </Field>
      <Field label={outlineLabel}>
        <div className="outline-field">
          <ColorField
            label={outlineLabel}
            presets={textSwatches(t)}
            none={{ label: t({ id: 'text.outline.none', message: 'No outline' }) }}
            {...outline}
          />
          {step && (
            <Segmented
              label={t({ id: 'text.outline.width', message: 'Outline width' })}
              value={step}
              fill
              options={OUTLINE_STEPS.map((o) => ({ value: o.value, label: t(o.label) }))}
              onChange={(next) =>
                setStyle('text.outline.width.undo', (now) => ({
                  ...now,
                  outline: now.outline && { ...now.outline, width: outlineWidth(now.size, next) },
                }))
              }
            />
          )}
        </div>
      </Field>
      <Field label={shadowLabel} labelFor={shadowId}>
        <Toggle
          id={shadowId}
          checked={style.shadow}
          onCheckedChange={() =>
            void commitToggle(
              () => contentNow(item.id, content).style.shadow,
              (on) => (on ? 'text.shadow.add.undo' : 'text.shadow.remove.undo'),
              (on) => styleOps((now) => ({ ...now, shadow: on })),
              { id: item.id },
            )
          }
        />
      </Field>
      <PositionField item={item} format={format} primary={primary} />
      <AnimationField item={item} typewriter />
      <TimingField item={item} />
    </>
  );
}
