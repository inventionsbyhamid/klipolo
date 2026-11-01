import { cn } from '../../design/cn';
import type { Handle } from '../../lib/geometry';

/** The eight handles: the corners, and the middle of each edge. */
const HANDLES: Handle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

interface GizmoHandlesProps {
  /** A small layer: handles outside its box, grabbing only what they draw. */
  compact: boolean;
  /** A text, image or video layer: the rotate knob on its stem above the top edge. */
  rotatable: boolean;
}

/**
 * A selected layer's eight 10 px paper handles with a navy edge, and its rotate knob, drawn on its box (so they turn
 * with it). Corners and the top and bottom edges scale it; the left and right edges stretch a text box, and scale a
 * picture about its centre line; the knob turns it about its centre (`gizmoGesture`). `data-handle` tells the drag what
 * a press began on. Pointer only: the arrow keys move and resize the layer too (`useNudge`).
 */
export function GizmoHandles({ compact, rotatable }: GizmoHandlesProps) {
  return (
    <>
      {HANDLES.map((handle) => (
        <span
          key={handle}
          data-handle={handle}
          aria-hidden
          className={cn('gizmo-handle', `is-${handle}`, compact && 'is-out')}
        />
      ))}
      {rotatable && (
        <>
          <span aria-hidden className="gizmo-stem" />
          <span data-handle="rotate" aria-hidden className="gizmo-knob" />
        </>
      )}
    </>
  );
}
