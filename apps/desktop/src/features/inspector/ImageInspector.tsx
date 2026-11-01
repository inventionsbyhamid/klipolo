import { useLingui } from '@lingui/react/macro';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Item } from '../../bindings/Item';
import { layoutPatchFor, scaleAbout } from '../../lib/geometry';
import { resolveRect } from '../../lib/layout';
import { AnimationField, PositionField, TimingField } from './common';
import { EffectsButton } from './EffectsButton';
import { Field } from './fields';
import { layoutOps } from '../../state/latest';
import { ToolSlider } from './ToolSlider';

/** The model's largest corner radius (a share of the shorter side): fully round ends. */
const MAX_RADIUS = 0.5;
const percent = (v: number) => `${Math.round(v * 100)}%`;

/**
 * A picture's fields: Size, Opacity and Corner radius (`PictureFields`), Effects, Position, Animation
 * and Timing.
 */
export function ImageInspector({ item, format, primary }: { item: Item; format: FormatKind; primary: FormatKind }) {
  return (
    <>
      <PictureFields item={item} format={format} primary={primary} />
      <EffectsButton target={{ type: 'item', id: item.id }} effects={item.effects} />
      <PositionField item={item} format={format} primary={primary} />
      <AnimationField item={item} typewriter={false} />
      <TimingField item={item} />
    </>
  );
}

/**
 * The fields of a layer that shows a picture (an image or a video): Size (its box's width, a share of the frame's,
 * keeping its shape about its centre), Opacity and Corner radius. A drag of a slider is dry-run in the preview and
 * commits one change on release.
 */
export function PictureFields({ item, format, primary }: { item: Item; format: FormatKind; primary: FormatKind }) {
  const { t } = useLingui();
  const rect = resolveRect(item.layout, primary, format);
  const size = t({ id: 'image.size', message: 'Size' });
  const opacity = t({ id: 'media.opacity', message: 'Opacity' });
  const corners = t({ id: 'media.corners', message: 'Corner radius', comment: 'How round the corners are' });
  return (
    <>
      <Field label={size}>
        <ToolSlider
          label={size}
          // Named as a resize on the canvas names it: "Resize video layer", "Resize image".
          commitLabel={{
            id: 'gizmo.resize.undo',
            params: { layer: item.content.type === 'video' ? 'video' : 'image' },
          }}
          value={Math.round(rect.w * 100) / 100}
          min={0.05}
          max={1}
          step={0.01}
          format={percent}
          ops={(w) =>
            layoutOps(item, (layout) => {
              const now = resolveRect(layout, primary, format);
              const centre = { x: now.x + now.w / 2, y: now.y + now.h / 2 };
              return layoutPatchFor(layout, scaleAbout(now, w / now.w, centre), format, primary);
            })
          }
        />
      </Field>
      <Field label={opacity}>
        <ToolSlider
          label={opacity}
          commitLabel="media.opacity.undo"
          value={item.layout.opacity}
          min={0}
          max={1}
          step={0.01}
          format={percent}
          ops={(value) => layoutOps(item, { opacity: value })}
        />
      </Field>
      <Field label={t({ id: 'media.corners.short', message: 'Corners', comment: 'The corner radius field' })}>
        <ToolSlider
          label={corners}
          commitLabel="media.corners.undo"
          value={item.layout.radius}
          min={0}
          max={MAX_RADIUS}
          step={0.01}
          format={(radius) => percent(radius / MAX_RADIUS)}
          ops={(radius) => layoutOps(item, { radius })}
        />
      </Field>
    </>
  );
}
