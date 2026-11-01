import { useLingui } from '@lingui/react/macro';
import { ChevronLeft, ChevronRight, Diamond } from 'lucide-react';
import type { Ease } from '../../bindings/Ease';
import type { Item } from '../../bindings/Item';
import { IconButton } from '../../design/IconButton';
import { PROPERTY_KEYS, SPRING, propertyTimes, nextKey, previousKey, type MotionProperty } from '../../lib/keys';
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
  const { t } = useLingui();
  // The playhead in the layer's time; the row's buttons are off while it is outside the layer.
  const playheadUs = usePlayback((s) => s.playheadUs);
  const tUs = playheadUs - item.startUs;
  const inside = tUs >= 0 && tUs <= item.durationUs;
  const slack = keySlackUs();
  const names: Record<MotionProperty, string> = {
    position: t({ id: 'motion.position', message: 'Position' }),
    scale: t({ id: 'motion.scale', message: 'Scale' }),
    rotation: t({ id: 'motion.rotation', message: 'Rotation' }),
    opacity: t({ id: 'motion.opacity', message: 'Opacity' }),
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
  const motion = item.layout.motion;
  return (
    <Field label={t({ id: 'motion.label', message: 'Motion', comment: 'Keys set by hand on a layer' })} stack>
      <div className="motion-rows">
        {PROPERTIES.map((prop) => {
          const times = propertyTimes(motion, prop);
          const here = times.find((k) => Math.abs(k - tUs) <= slack);
          const before = previousKey(times, tUs, slack);
          const after = nextKey(times, tUs, slack);
          const key =
            here === undefined ? undefined : motion?.keys?.[PROPERTY_KEYS[prop][0]]?.find((k) => Math.abs(k.tUs - here) <= slack);
          const choice = easeChoice(key?.ease);
          const name = names[prop];
          return (
            <div key={prop} className="motion-row" data-property={prop}>
              <span className="motion-name">{name}</span>
              <IconButton
                className="icon-btn-sm"
                icon={ChevronLeft}
                label={t({ id: 'motion.previous', message: `Previous ${name} key`, comment: '{name} is Position, Scale…' })}
                disabled={before === null}
                onClick={() => before !== null && seek(before)}
              />
              <IconButton
                className="icon-btn-sm motion-key"
                icon={<Diamond size={14} strokeWidth={2} aria-hidden fill={here !== undefined ? 'currentColor' : 'none'} />}
                label={
                  here !== undefined
                    ? t({ id: 'motion.key.remove', message: `Remove ${name} key`, comment: '{name} is Position, Scale…' })
                    : t({ id: 'motion.key.add', message: `Add ${name} key`, comment: '{name} is Position, Scale…' })
                }
                active={here !== undefined}
                disabled={!inside}
                onClick={() => void toggleKey(item, prop, here ?? tUs)}
              />
              <IconButton
                className="icon-btn-sm"
                icon={ChevronRight}
                label={t({ id: 'motion.next', message: `Next ${name} key`, comment: '{name} is Position, Scale…' })}
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
