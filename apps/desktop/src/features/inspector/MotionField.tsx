import { useLingui } from '@lingui/react/macro';
import { ChevronLeft, ChevronRight, Diamond } from 'lucide-react';
import type { Ease } from '../../bindings/Ease';
import type { Item } from '../../bindings/Item';
import { IconButton } from '../../design/IconButton';
import { PROPERTY_KEYS, SPRING, propertyTimes, nextKey, previousKey, type MotionProperty } from '../../lib/keys';
import { countRender } from '../../lib/renderCount';
import { useShallow } from 'zustand/react/shallow';
import { usePlayback } from '../../state/playback';
import { keySlackUs, setKeyEase, toggleKey } from '../../state/keyEdits';
import { Field, Select } from './fields';

/** The eases the card offers; a key set elsewhere with another shows as Other. */
type EaseChoice = 'linear' | 'easeIn' | 'easeOut' | 'easeInOut' | 'spring' | 'other';

function easeChoice(ease: Ease | undefined): EaseChoice {
  if (ease === undefined || ease === 'linear') return 'linear';
  if (ease === 'easeIn' || ease === 'easeOut' || ease === 'easeInOut') return ease;
  if (typeof ease === 'object' && 'spring' in ease) return 'spring';
  return 'other';
}

const PROPERTIES: MotionProperty[] = ['position', 'scale', 'rotation', 'opacity'];

/**
 * Motion (keys set by hand): Position, Scale, Rotation and Opacity, each with its key button at the playhead (a key
 * holding the value there, or the key there taken away), the keys before and after it, and the ease into its key at the
 * playhead. With keys, moving, resizing or turning the layer on the canvas and its Size and Opacity set the key at the
 * playhead; without them they set the layer as before.
 */
export function MotionField({ item }: { item: Item }) {
  countRender(`motion:${item.id}`);
  // An adjustment layer's box stays upright (the engine draws it so): it has no Rotation.
  const properties = item.content.type === 'adjustment' ? PROPERTIES.filter((p) => p !== 'rotation') : PROPERTIES;
  const { t } = useLingui();
  const slack = keySlackUs();
  const motion = item.layout.motion;
  // What the rows show at the playhead (in the layer's time): whether it is inside the layer, and each row's key there,
  // and the keys before and after it. Only a change of those renders the card again, not every frame of playback.
  const view = usePlayback(
    useShallow((s) => {
      const t = s.playheadUs - item.startUs;
      const out: (number | boolean | null)[] = [t >= 0 && t <= item.durationUs];
      for (const prop of properties) {
        const times = propertyTimes(motion, prop);
        out.push(times.find((k) => Math.abs(k - t) <= slack) ?? null, previousKey(times, t, slack), nextKey(times, t, slack));
      }
      return out;
    }),
  );
  const inside = view[0] as boolean;
  const now = () => usePlayback.getState().playheadUs - item.startUs;
  const names: Record<MotionProperty, string> = {
    position: t({ id: 'motion.position', message: 'Position' }),
    scale: t({ id: 'motion.scale', message: 'Scale' }),
    rotation: t({ id: 'motion.rotation', message: 'Rotation' }),
    opacity: t({ id: 'motion.opacity', message: 'Opacity' }),
  };
  // The same names inside a sentence: "Add position key".
  const nouns: Record<MotionProperty, string> = {
    position: t({ id: 'motion.position.noun', message: 'position', comment: 'Inside “Add position key”' }),
    scale: t({ id: 'motion.scale.noun', message: 'scale', comment: 'Inside “Add scale key”' }),
    rotation: t({ id: 'motion.rotation.noun', message: 'rotation', comment: 'Inside “Add rotation key”' }),
    opacity: t({ id: 'motion.opacity.noun', message: 'opacity', comment: 'Inside “Add opacity key”' }),
  };
  const eases: { value: EaseChoice; label: string }[] = [
    { value: 'linear', label: t({ id: 'motion.ease.linear', message: 'Linear' }) },
    { value: 'easeIn', label: t({ id: 'motion.ease.in', message: 'Ease in' }) },
    { value: 'easeOut', label: t({ id: 'motion.ease.out', message: 'Ease out' }) },
    { value: 'easeInOut', label: t({ id: 'motion.ease.inOut', message: 'Ease in and out' }) },
    { value: 'spring', label: t({ id: 'motion.ease.spring', message: 'Spring' }) },
  ];
  const other = { value: 'other' as const, label: t({ id: 'motion.ease.other', message: 'Other', comment: 'An ease set from chat' }) };
  const seek = (layerUs: number) => usePlayback.getState().seek(item.startUs + layerUs);
  return (
    <Field label={t({ id: 'motion.label', message: 'Motion', comment: 'Keys set by hand on a layer' })} stack>
      <div className="motion-rows">
        {properties.map((prop, i) => {
          const here = (view[1 + 3 * i] as number | null) ?? undefined;
          const before = view[2 + 3 * i] as number | null;
          const after = view[3 + 3 * i] as number | null;
          // The key there (Position's x key, else its y key) gives the ease shown.
          const key =
            here === undefined
              ? undefined
              : PROPERTY_KEYS[prop]
                  .map((name) => motion?.keys?.[name]?.find((k) => Math.abs(k.tUs - here) <= slack))
                  .find((k) => k !== undefined);
          const choice = easeChoice(key?.ease);
          const name = names[prop];
          const noun = nouns[prop];
          return (
            <div key={prop} className="motion-row" data-property={prop}>
              <span className="motion-name">{name}</span>
              <IconButton
                className="icon-btn-sm"
                icon={ChevronLeft}
                label={t({ id: 'motion.previous', message: `Previous ${noun} key`, comment: '{noun} is position, scale…' })}
                disabled={before === null}
                onClick={() => before !== null && seek(before)}
              />
              <IconButton
                className="icon-btn-sm motion-key"
                icon={<Diamond size={14} strokeWidth={2} aria-hidden fill={here !== undefined ? 'currentColor' : 'none'} />}
                label={
                  here !== undefined
                    ? t({ id: 'motion.key.remove', message: `Remove ${noun} key`, comment: '{noun} is position, scale…' })
                    : t({ id: 'motion.key.add', message: `Add ${noun} key`, comment: '{noun} is position, scale…' })
                }
                active={here !== undefined}
                disabled={!inside}
                onClick={() => void toggleKey(item, prop, here ?? now())}
              />
              <IconButton
                className="icon-btn-sm"
                icon={ChevronRight}
                label={t({ id: 'motion.next', message: `Next ${noun} key`, comment: '{noun} is position, scale…' })}
                disabled={after === null}
                onClick={() => after !== null && seek(after)}
              />
              {here !== undefined ? (
                <Select<EaseChoice>
                  label={t({ id: 'motion.ease', message: `${name} ease`, comment: '{name} is Position, Scale…' })}
                  value={choice}
                  choices={choice === 'other' ? [...eases, other] : eases}
                  onPick={(v) => {
                    if (v === 'other') return;
                    void setKeyEase(item, prop, here, v === 'spring' ? SPRING : v);
                  }}
                />
              ) : (
                <span className="motion-ease-none" />
              )}
            </div>
          );
        })}
      </div>
    </Field>
  );
}
