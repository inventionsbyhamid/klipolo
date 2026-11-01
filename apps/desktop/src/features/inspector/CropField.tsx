import { useLingui } from '@lingui/react/macro';
import { useId, useState } from 'react';
import type { Crop } from '../../bindings/Crop';
import type { Op } from '../../bindings/Op';
import { Button } from '../../design/Button';
import { Icon } from '../../layout/Icon';
import { commitLatest } from '../../state/latest';
import { Field } from './fields';
import { ToolSlider } from './ToolSlider';

/** The least share of a picture a crop keeps, across and down (`klipolo_model::crop::MIN_KEPT`). */
export const MIN_KEPT = 0.1;
export const NO_CROP: Crop = { left: 0, top: 0, right: 0, bottom: 0 };
/** The most an edge's slider shows: the whole picture but the tenth that is always kept. */
const MAX_EDGE = 1 - MIN_KEPT;

type Edge = keyof Crop;
const EDGES: Edge[] = ['left', 'right', 'top', 'bottom'];
const OPPOSITE: Record<Edge, Edge> = { left: 'right', right: 'left', top: 'bottom', bottom: 'top' };
const percent = (v: number) => `${Math.round(v * 100)}%`;

/** Whether `crop` cuts nothing (or there is none). */
export function isNoCrop(crop: Crop | null | undefined): boolean {
  return !crop || EDGES.every((edge) => crop[edge] === 0);
}

/** How far `edge` of `crop` may go: what its opposite edge leaves, less the tenth that is always kept. */
function room(crop: Crop, edge: Edge): number {
  return Math.max(0, Math.round((1 - MIN_KEPT - crop[OPPOSITE[edge]]) * 100) / 100);
}

/** `crop` with `edge` at `value`, held between 0 and its room. */
export function withEdge(crop: Crop, edge: Edge, value: number): Crop {
  return { ...crop, [edge]: Math.min(Math.max(value, 0), room(crop, edge)) };
}

/**
 * Crop (a clip's footage, a video layer's picture): the edges cut off the picture as it is shown, each a share of its
 * width or height, always leaving a tenth of it each way. Its row says whether it is cropped and opens the Left, Right,
 * Top and Bottom sliders (open from the start while it is), with Reset crop while it is. Every slider spans 0 to 90%,
 * so a knob never jumps as the other edge moves; what goes in is held so a tenth stays (`withEdge`), and a drag held
 * at what the edge has changes nothing. A drag is dry-run in the preview and commits one undo step on release, built
 * on the crop as the project has it then (`latest`), so another edge's change on its way stays. `ops` makes the
 * change; a crop of nothing takes it off.
 */
export function CropField({
  crop,
  latest,
  ops,
}: {
  crop: Crop | null | undefined;
  latest: () => Crop | null | undefined;
  ops: (crop: Crop) => Op[];
}) {
  const { t } = useLingui();
  const cropped = !isNoCrop(crop);
  const [open, setOpen] = useState(cropped);
  const edges = useId();
  /** The change a slider at `value` makes to `edge`, on the crop as the project has it then; none when held as it is. */
  const edgeOps = (edge: Edge, value: number) => {
    const now = latest() ?? NO_CROP;
    const next = withEdge(now, edge, value);
    return next[edge] === now[edge] ? [] : ops(next);
  };
  const shown = crop ?? NO_CROP;
  const names: Record<Edge, [string, string]> = {
    left: [
      t({ id: 'crop.left', message: 'Left', comment: 'How much is cut off the left edge' }),
      t({ id: 'crop.left.named', message: 'Crop left' }),
    ],
    right: [
      t({ id: 'crop.right', message: 'Right', comment: 'How much is cut off the right edge' }),
      t({ id: 'crop.right.named', message: 'Crop right' }),
    ],
    top: [
      t({ id: 'crop.top', message: 'Top', comment: 'How much is cut off the top edge' }),
      t({ id: 'crop.top.named', message: 'Crop top' }),
    ],
    bottom: [
      t({ id: 'crop.bottom', message: 'Bottom', comment: 'How much is cut off the bottom edge' }),
      t({ id: 'crop.bottom.named', message: 'Crop bottom' }),
    ],
  };
  const label = t({ id: 'crop', message: 'Crop', comment: 'Cut the edges off a video' });
  const state = cropped
    ? t({ id: 'crop.on', message: 'Cropped', comment: 'The video has edges cut off' })
    : t({ id: 'crop.none', message: 'None', comment: 'Nothing cut off the video' });
  return (
    <>
      <Field label={label}>
        <button
          type="button"
          className="select crop-toggle"
          aria-expanded={open}
          aria-controls={edges}
          aria-label={`${label}: ${state}`}
          onClick={() => setOpen((o) => !o)}
        >
          <span className="select-text">{state}</span>
          <Icon name="chev-down" />
        </button>
      </Field>
      {open && (
        <div id={edges} className="field field-col" role="group" aria-label={label}>
          {EDGES.map((edge) => (
            <Field key={edge} label={names[edge][0]}>
              <ToolSlider
                label={names[edge][1]}
                commitLabel="crop.undo"
                value={shown[edge]}
                min={0}
                max={MAX_EDGE}
                step={0.01}
                format={percent}
                ops={(value) => edgeOps(edge, value)}
              />
            </Field>
          ))}
          {cropped && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void commitLatest('crop.reset.undo', () => ops(NO_CROP))}
            >
              {t({ id: 'crop.reset', message: 'Reset crop' })}
            </Button>
          )}
        </div>
      )}
    </>
  );
}
