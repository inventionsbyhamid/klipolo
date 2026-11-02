import { useLingui } from '@lingui/react/macro';
import { useRef } from 'react';
import type { BoxN } from '../../bindings/BoxN';
import type { FocusKind } from '../../bindings/FocusKind';
import type { Item } from '../../bindings/Item';
import type { ItemContent } from '../../bindings/ItemContent';
import type { ZoomTarget } from '../../bindings/ZoomTarget';
import { Segmented } from '../../design/Segmented';
import { Tooltip } from '../../design/Tooltip';
import { useIntlLocale } from '../../i18n/react';
import { formatList, formatNumber } from '../../lib/format';
import { itemOp } from '../../lib/ops';
import { commitLatest, contentNow } from '../../state/latest';
import { TimingField } from './common';
import { Field } from './fields';
import { ToolSlider } from './ToolSlider';

export type ZoomContent = Extract<ItemContent, { type: 'zoom' }>;

/** The zoom the slider offers (the model allows 1 to 5x). */
const MIN_SCALE = 1.1;
const MAX_SCALE = 4;

const percent = (v: number) => `${Math.round(v * 100)}%`;

/**
 * The area a zoom of `scale` shows, `1 / scale` of the frame on each side, centred at `cx`, `cy` and kept inside the
 * frame. The engine zooms to its centre.
 */
export function zoomArea(scale: number, cx = 0.5, cy = 0.5): BoxN {
  const side = 1 / Math.max(1, scale);
  const at = (c: number) => Math.min(1 - side, Math.max(0, c - side / 2));
  return { x: at(cx), y: at(cy), w: side, h: side };
}

/** Where the area sits along one axis: 0 at the left (or top) edge, 1 at the right (or bottom), as the engine reads it. */
function travel(start: number, side: number): number {
  return side >= 1 ? 0.5 : Math.min(1, Math.max(0, start / (1 - side)));
}

/**
 * A zoom layer's fields (made by a recording or by chat): how far it zooms (1.1 to 4x), and where: Smart (the camera
 * plans it from clicks, pauses and typing; one chat made from some of those alone says so, `SmartNote`) or an Area,
 * moved across and down by two sliders and always inside the frame; then its Timing. Each slider gesture is one undo
 * step, dry-run as it moves.
 */
export function ZoomInspector({ item, content }: { item: Item; content: ZoomContent }) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const label = 'zoom.undo';
  const op = (next: ZoomContent) => [itemOp(item, { content: next })];
  /** The content as the project has it when a change goes in, so a quick second change keeps the first. */
  const now = () => contentNow(item.id, content);
  /** The area an Area zoom, or a slow push-in chat made, goes to. */
  const rectOf = (c: ZoomContent) => (c.target.type === 'rect' || c.target.type === 'push' ? c.target.rect : null);
  /** `c`'s target with its area moved to `rect`: a push stays a push, with its ramp. */
  const withRect = (c: ZoomContent, rect: BoxN): ZoomTarget =>
    c.target.type === 'push' ? { ...c.target, rect } : { type: 'rect', rect };
  const rect = rectOf(content);
  /** The smart target this zoom had last (chat may have narrowed what it plans from): Smart puts it back. */
  const lastSmart = useRef<ZoomTarget>({ type: 'auto' });
  if (content.target.type === 'auto') lastSmart.current = content.target;
  const centre = (r: BoxN) => [r.x + r.w / 2, r.y + r.h / 2] as const;
  /** The area kept round its centre at the new scale. */
  const scaled = (c: ZoomContent, scale: number): ZoomTarget => {
    const area = rectOf(c);
    return area ? withRect(c, zoomArea(scale, ...centre(area))) : c.target;
  };
  const along = (axis: 'x' | 'y', value: number): ZoomContent => {
    const c = now();
    const area = rectOf(c);
    if (!area) return c;
    const side = axis === 'x' ? area.w : area.h;
    return { ...c, target: withRect(c, { ...area, [axis]: value * Math.max(0, 1 - side) }) };
  };
  const zoom = t({ id: 'zoom.scale', message: 'Zoom', comment: 'How far a zoom layer zooms in' });
  const target = t({ id: 'zoom.target', message: 'Target', comment: 'Where a zoom layer zooms to' });
  return (
    <>
      <Field label={zoom}>
        <ToolSlider
          label={zoom}
          commitLabel={label}
          value={content.scale}
          // A zoom set outside the slider's usual range (chat's 5x) shows as it is.
          min={Math.min(MIN_SCALE, content.scale)}
          max={Math.max(MAX_SCALE, content.scale)}
          step={0.1}
          format={(v) => `${formatNumber(locale, v, { maximumFractionDigits: 1 })}×`}
          ops={(scale) => {
            const c = now();
            return op({ ...c, scale, target: scaled(c, scale) });
          }}
        />
      </Field>
      <Field label={target}>
        <Segmented
          label={target}
          value={rect ? 'area' : 'smart'}
          fill
          options={[
            {
              value: 'smart',
              label: t({
                id: 'zoom.target.smart',
                message: 'Smart',
                comment: 'A zoom that plans where to go from the recording: clicks, pauses and typing',
              }),
              tooltip: t({ id: 'zoom.target.smart.tip', message: 'Zooms where you click, pause or type' }),
            },
            { value: 'area', label: t({ id: 'zoom.target.area', message: 'Area', comment: 'Zoom to an area you place' }) },
          ]}
          onChange={(mode) =>
            void commitLatest(label, () => {
              const c = now();
              const smart = c.target.type === 'auto' ? c.target : lastSmart.current;
              return op({ ...c, target: mode === 'area' ? { type: 'rect', rect: zoomArea(c.scale) } : smart });
            })
          }
        />
      </Field>
      {content.target.type === 'auto' && <SmartNote using={content.target.using ?? []} />}
      {rect && (
        <>
          <Field label={t({ id: 'zoom.area.across.short', message: 'Across', comment: 'The zoom area, left to right' })}>
            <ToolSlider
              label={t({ id: 'zoom.area.across', message: 'Area across', comment: 'Slider: the zoom area, left to right' })}
              commitLabel={label}
              value={travel(rect.x, rect.w)}
              min={0}
              max={1}
              step={0.01}
              format={percent}
              ops={(v) => op(along('x', v))}
            />
          </Field>
          <Field label={t({ id: 'zoom.area.down.short', message: 'Down', comment: 'The zoom area, top to bottom' })}>
            <ToolSlider
              label={t({ id: 'zoom.area.down', message: 'Area down', comment: 'Slider: the zoom area, top to bottom' })}
              commitLabel={label}
              value={travel(rect.y, rect.h)}
              min={0}
              max={1}
              step={0.01}
              format={percent}
              ops={(v) => op(along('y', v))}
            />
          </Field>
        </>
      )}
      <TimingField item={item} />
    </>
  );
}

const KINDS: FocusKind[] = ['clicks', 'pauses', 'typing'];

/**
 * What a smart zoom plans from when chat made it from some kinds of action alone (`ZoomTarget::Auto { using }`), and
 * what that means: a part where such a zoom would hide another kind of action stays in full view. Nothing for a zoom
 * that uses every kind (the default).
 */
export function SmartNote({ using }: { using: FocusKind[] }) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const kinds = KINDS.filter((k) => using.includes(k));
  if (kinds.length === 0 || kinds.length === KINDS.length) return null;
  const word: Record<FocusKind, string> = {
    clicks: t({ id: 'zoom.kind.clicks', message: 'clicks', comment: 'What a smart zoom plans from' }),
    pauses: t({ id: 'zoom.kind.pauses', message: 'pauses', comment: 'Where the mouse stops moving' }),
    typing: t({ id: 'zoom.kind.typing', message: 'typing', comment: 'What a smart zoom plans from' }),
  };
  const only = formatList(
    locale,
    kinds.map((k) => word[k]),
  );
  const others = new Intl.ListFormat(locale, { type: 'disjunction' }).format(
    KINDS.filter((k) => !kinds.includes(k)).map((k) => word[k]),
  );
  const why = t({
    id: 'zoom.smart.narrowed',
    message: `This zoom plans only from ${only}. Where it would hide ${others}, that part stays in full view.`,
    comment: '{only} and {others} are lists of clicks, pauses and typing',
  });
  return (
    <Tooltip label={why} describes>
      <span tabIndex={0} role="note" className="field-note">
        {t({ id: 'zoom.smart.only', message: `Only ${only}`, comment: '{only} is a list, e.g. “pauses”' })}
      </span>
    </Tooltip>
  );
}
