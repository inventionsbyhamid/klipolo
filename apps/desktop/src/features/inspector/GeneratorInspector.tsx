import type { Item } from '../../bindings/Item';
import { BUILTINS, contribLabel } from '../../lib/contrib';
import { itemOp } from '../../lib/ops';
import { contentNow } from '../../state/latest';
import { TimingField } from './common';
import { effectLabels } from './effectOps';
import { EffectsButton } from './EffectsButton';
import { ParamFields } from './ParamFields';
import type { GeneratorContent } from './TemplateInspector';

/**
 * A generator layer's fields (a gradient, waves, film grain): its settings, generated from its contribution's schema,
 * each with Reset once it differs from its default; its effects; and its Timing. One this build lacks shows "Missing
 * generator" (its id as a tooltip). Each change is one undo step, built from the layer as the project has it then.
 */
export function GeneratorInspector({ item, content }: { item: Item; content: GeneratorContent }) {
  const def = BUILTINS.find((d) => d.id === content.source && d.kind === 'generator');
  /** The content as the project has it when a change goes in, so a quick second change keeps the first. */
  const now = () => contentNow(item.id, content);
  const withParams = (params: GeneratorContent['params']) => [itemOp(item, { content: { ...now(), params } })];
  return (
    <>
      {def ? (
        <ParamFields
          def={def}
          values={content.params}
          opsFor={(key, value) => withParams({ ...now().params, [key]: value })}
          resetOps={(key) => {
            const params = { ...now().params };
            delete params[key];
            return withParams(params);
          }}
          valuesNow={() => now().params}
          commitLabel={effectLabels.change(content.source)}
        />
      ) : (
        <p title={content.source} className="field-note">
          {contribLabel(content.source, 'generator')}
        </p>
      )}
      <EffectsButton target={{ type: 'item', id: item.id }} effects={item.effects} />
      <TimingField item={item} />
    </>
  );
}
