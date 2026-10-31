import { useDragFeedback } from './dragFeedback';

/** The lines a moving layer has snapped to, across the whole canvas: dashed paper. */
export function SnapGuides() {
  const guides = useDragFeedback((s) => s.guides);
  return guides.map((g) => (
    <div
      key={`${g.axis}-${g.at}`}
      aria-hidden
      data-testid="snap-guide"
      className={g.axis === 'x' ? 'snap-guide is-x' : 'snap-guide is-y'}
      style={g.axis === 'x' ? { left: `${g.at * 100}%` } : { top: `${g.at * 100}%` }}
    />
  ));
}
