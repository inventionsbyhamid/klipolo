import { useLingui } from '@lingui/react/macro';
import { useState } from 'react';
import type { Item } from '../../bindings/Item';
import type { ItemContent } from '../../bindings/ItemContent';
import type { Op } from '../../bindings/Op';
import type { ParamSpec } from '../../bindings/ParamSpec';
import { BUILTINS } from '../../lib/contrib';
import { clearPreview, dropPendingPreview, previewOps } from '../../state/livePreview';
import { itemOp } from '../../lib/ops';
import { TimingField } from './common';
import { MotionField } from './MotionField';
import { EffectsButton } from './EffectsButton';
import { Field, Swatches, type Swatch } from './fields';
import { commitLatest, contentNow } from '../../state/latest';

export type GeneratorContent = Extract<ItemContent, { type: 'generator' }>;

/** A title template's settings (`contrib.json`'s params): its words and its accent. */
export function templateParams(content: GeneratorContent): ParamSpec[] {
  return BUILTINS.find((def) => def.id === content.source)?.params ?? [];
}

/** The accent colours offered: the brand yellow first (the templates' own), then the lane colours and white. */
function accents(t: (d: { id: string; message: string }) => string): Swatch[] {
  return [
    { value: '#FFD02E', name: t({ id: 'color.yellow', message: 'Yellow' }) },
    { value: '#FFFFFF', name: t({ id: 'color.white', message: 'White' }) },
    { value: '#7F92DA', name: t({ id: 'color.blue', message: 'Blue' }) },
    { value: '#3FB8AF', name: t({ id: 'color.teal', message: 'Teal' }) },
    { value: '#9A88EC', name: t({ id: 'color.violet', message: 'Violet' }) },
  ];
}

/** One of a template's text settings: typed with the preview showing each change at once, committed on Enter or leave. */
function WordsField({
  item,
  content,
  spec,
  first,
  locked,
}: {
  item: Item;
  content: GeneratorContent;
  spec: Extract<ParamSpec, { type: 'text' }>;
  first: boolean;
  locked: boolean;
}) {
  const saved = typeof content.params[spec.key] === 'string' ? (content.params[spec.key] as string) : spec.default;
  const [draft, setDraft] = useState<string | null>(null);
  const ops = (text: string): Op[] => {
    const now = contentNow(item.id, content);
    return [itemOp(item, { content: { ...now, params: { ...now.params, [spec.key]: text } } })];
  };
  const finish = () => {
    if (draft === null) return;
    const text = draft;
    setDraft(null);
    if (text === saved) {
      clearPreview();
      return;
    }
    // The new revision replaces the dry run: no flicker.
    dropPendingPreview();
    void commitLatest('title.edit.undo', () => ops(text)).then((ok) => {
      if (!ok) clearPreview();
    });
  };
  return (
    <Field label={spec.label} labelFor={`template-${item.id}-${spec.key}`}>
      <input
        id={`template-${item.id}-${spec.key}`}
        className="input input-sm"
        data-inspector-text={first ? '' : undefined}
        value={draft ?? saved}
        maxLength={spec.maxLen}
        readOnly={locked}
        onChange={(e) => {
          const text = e.target.value;
          setDraft(text);
          // The preview shows the words as they are typed.
          previewOps(() => ops(text));
        }}
        onBlur={finish}
        onKeyDown={(e) => {
          // Typing here never reaches the editor's shortcuts, and the input method owns Return and Escape while a
          // composition is on.
          e.stopPropagation();
          if (e.nativeEvent.isComposing || e.keyCode === 229) return;
          if (e.key === 'Enter') {
            e.preventDefault();
            e.currentTarget.blur();
          } else if (e.key === 'Escape') {
            // The saved words back, in the field and the preview: nothing is committed as it lets go.
            e.preventDefault();
            setDraft(null);
            clearPreview();
          }
        }}
      />
    </Field>
  );
}

/**
 * A title template's fields (the lower third, the title card): its words (Name and Role, or Title and Subtitle), its
 * accent colour, its effects and its timing. Its place is the frame, so it has no position or size. The words are typed here, the
 * preview showing each change; Enter or leaving the field makes it one undo step.
 */
export function TemplateInspector({
  item,
  content,
  locked,
}: {
  item: Item;
  content: GeneratorContent;
  locked: boolean;
}) {
  const { t } = useLingui();
  const specs = templateParams(content);
  const firstText = specs.find((s) => s.type === 'text')?.key;
  return (
    <>
      {specs.map((spec) => {
        if (spec.type === 'text')
          return (
            <WordsField
              key={spec.key}
              item={item}
              content={content}
              spec={spec}
              first={spec.key === firstText}
              locked={locked}
            />
          );
        if (spec.type === 'color') {
          const value = typeof content.params[spec.key] === 'string' ? (content.params[spec.key] as string) : spec.default;
          return (
            <Field key={spec.key} label={spec.label}>
              <Swatches
                label={spec.label}
                value={value}
                swatches={accents(t)}
                onPick={(color) =>
                  void commitLatest('title.accent.undo', () => {
                    const now = contentNow(item.id, content);
                    return [itemOp(item, { content: { ...now, params: { ...now.params, [spec.key]: color } } })];
                  })
                }
              />
            </Field>
          );
        }
        return null;
      })}
      <EffectsButton target={{ type: 'item', id: item.id }} effects={item.effects} />
      <MotionField item={item} />
      <TimingField item={item} />
    </>
  );
}
