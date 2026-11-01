import { useLingui } from '@lingui/react/macro';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Item } from '../../bindings/Item';
import type { ItemContent } from '../../bindings/ItemContent';
import type { TextStyle } from '../../bindings/TextStyle';
import { Segmented } from '../../design/Segmented';
import type { Label } from '../../lib/labels';
import { useBundledFamilies } from '../../lib/fonts';
import { scaledStyle } from '../../lib/textLook';
import { itemOp } from '../../lib/ops';
import { AnimationField, PositionField, TimingField } from './common';
import { EffectsButton } from './EffectsButton';
import { Field, Select, Swatches, type Swatch } from './fields';
import { backdropOf, backdropStyle, type Backdrop } from './inspectorOps';
import { commitLatest, contentNow } from '../../state/latest';
import { TextField } from './TextField';
import { ToolSlider } from './ToolSlider';

type TextContent = Extract<ItemContent, { type: 'text' }>;

/** The size slider's range: up to the largest size the model takes (`model.text.size`, 4 to 400); under 8 none reads. */
export const MIN_TEXT_SIZE = 8;
export const MAX_TEXT_SIZE = 400;

/**
 * The 12 bundled families a text can use, in the Font menu's order (`klipolo_fonts::bundled::BUNDLED`): the menu shows
 * these until Rust's own list (`fonts_bundled`) is in.
 */
export const FAMILIES = [
  'Inter',
  'TikTok Sans',
  'Montserrat',
  'Anton',
  'Bangers',
  'Oswald',
  'Rubik',
  'Bebas Neue',
  'Luckiest Guy',
  'Playfair Display',
  'Caveat',
  'JetBrains Mono',
];

/** The text colours offered, then "+" for any other. */
export function textSwatches(t: (d: { id: string; message: string }) => string): Swatch[] {
  return [
    { value: '#FFFFFF', name: t({ id: 'color.white', message: 'White' }) },
    { value: '#121A33', name: t({ id: 'color.navy', message: 'Navy' }) },
    { value: '#FFD02E', name: t({ id: 'color.yellow', message: 'Yellow' }) },
    { value: '#7F92DA', name: t({ id: 'color.blue', message: 'Blue' }) },
    { value: '#000000', name: t({ id: 'color.black', message: 'Black' }) },
  ];
}

/**
 * A text layer's fields: its words, Font (the 12 bundled families), Size, Color, Background (None, Box, 
 * Shadow), Effects, Position, Animation and Timing. Each change is one undo step, built at its turn from the title as the
 * project has it then (`contentNow`), so a change on its way (words just finished on the canvas) is kept.
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
  const styleOps = (change: (now: TextStyle) => TextStyle) => {
    const now = contentNow(item.id, content);
    return [itemOp(item, { content: { ...now, style: change(now.style) } })];
  };
  const setStyle = (label: Label, change: (now: TextStyle) => TextStyle) =>
    void commitLatest(label, () => styleOps(change));
  const backgrounds: { value: Backdrop; label: string }[] = [
    { value: 'none', label: t({ id: 'text.background.none', message: 'None' }) },
    { value: 'box', label: t({ id: 'text.background.box', message: 'Box' }) },
    { value: 'shadow', label: t({ id: 'text.background.shadow', message: 'Shadow' }) },
  ];
  const sizeLabel = t({ id: 'text.size', message: 'Size' });
  const families = useBundledFamilies(FAMILIES);
  const colorLabel = t({ id: 'text.color', message: 'Text color' });
  return (
    <>
      <TextField key={content.text} id={item.id} content={content} locked={locked} />
      <Field label={t({ id: 'text.font', message: 'Font' })}>
        <Select
          label={t({ id: 'text.font', message: 'Font' })}
          value={style.font.family}
          choices={(families.includes(style.font.family) ? families : [style.font.family, ...families]).map(
            (family) => ({ value: family, label: family }),
          )}
          onPick={(family) =>
            setStyle('text.font.undo', (now) => ({ ...now, font: { family, source: { type: 'bundled' } } }))
          }
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
      <Field label={t({ id: 'text.color.short', message: 'Color', comment: 'The text color field' })}>
        <Swatches
          label={colorLabel}
          value={style.color}
          swatches={textSwatches(t)}
          onPick={(color) => setStyle('text.color.undo', (now) => ({ ...now, color }))}
        />
      </Field>
      <Field label={t({ id: 'text.background', message: 'Background' })} narrowStack>
        <Segmented
          label={t({ id: 'text.background', message: 'Background' })}
          value={backdropOf(style)}
          options={backgrounds}
          fill
          onChange={(backdrop) => setStyle('text.background.undo', (now) => backdropStyle(now, backdrop))}
        />
      </Field>
      <EffectsButton target={{ type: 'item', id: item.id }} effects={item.effects} />
      <PositionField item={item} format={format} primary={primary} />
      <AnimationField item={item} typewriter />
      <TimingField item={item} />
    </>
  );
}
