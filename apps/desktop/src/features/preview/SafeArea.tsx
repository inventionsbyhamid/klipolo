import type { FormatKind } from '../../bindings/FormatKind';
import { dragArea, safeArea } from '../../lib/safeAreas';
import { useUi } from '../../state/ui';
import { useDragFeedback } from './dragFeedback';

/** Corners stay round on the stretched 9:16 view box (one unit across is 9/16 of one unit down). */
const CORNER = 0.01;

/**
 * In 9:16, the apps' own buttons, caption and bars as a silhouette over the video, with a dashed edge around what
 * stays clear. It shows all the time while "Show safe area" is on; while a layer is dragged it shows the area chosen
 * last, else all platforms. A view of the preview only: never in the project or an export.
 */
export function SafeArea({ format }: { format: FormatKind }) {
  const dragging = useDragFeedback((s) => s.dragging);
  const chosen = useUi((s) => s.safeArea);
  const last = useUi((s) => s.lastSafeArea);
  if (format !== 'portrait' || (chosen === 'off' && !dragging)) return null;
  const spec = safeArea(dragArea(chosen, last));
  if (!spec) return null;
  const { clear } = spec;
  return (
    <svg
      aria-hidden
      data-testid="safe-area"
      data-platform={spec.id}
      className="safe-area"
      viewBox="0 0 1 1"
      preserveAspectRatio="none"
    >
      {/* Each shape is filled with its own alpha. No group opacity: that would paint the overlay through a
          transparency layer on every repaint under a moving layer, and the shapes never overlap. */}
      <g className="safe-area-shapes">
        {spec.shapes.map(({ rect, round }, i) =>
          round ? (
            <ellipse
              key={i}
              cx={rect.x + rect.w / 2}
              cy={rect.y + rect.h / 2}
              rx={rect.w / 2}
              ry={rect.h / 2}
            />
          ) : (
            <rect
              key={i}
              x={rect.x}
              y={rect.y}
              width={rect.w}
              height={rect.h}
              rx={CORNER}
              ry={(CORNER * 9) / 16}
            />
          ),
        )}
      </g>
      <rect
        x={clear.x}
        y={clear.y}
        width={clear.w}
        height={clear.h}
        className="safe-area-clear"
        strokeWidth={1}
        strokeDasharray="4 3"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}
