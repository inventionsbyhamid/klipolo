import { cn } from '../../design/cn';
import type { Handle } from '../../lib/geometry';

/** The eight handles: the corners, and the middle of each edge. */
const HANDLES: Handle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

interface GizmoHandlesProps {
  /** Text: its left and right edges change its wrap width. Elsewhere they keep the shape, as the corners do. */
  sides: boolean;
  /** A small layer: handles outside its box, grabbing only what they draw. */
  compact: boolean;
}

/**
 * A selected layer's eight 10 px paper handles with a navy edge, drawn on its box (so they turn with it). Corners and
 * the top and bottom edges scale it; the left and right edges stretch a text box (and scale a picture). `data-handle`
 * tells the drag what a press began on. Pointer only: the arrow keys move and resize the layer too (`useNudge`).
 */
export function GizmoHandles({ sides, compact }: GizmoHandlesProps) {
  return HANDLES.map((handle) => (
    <span
      key={handle}
      data-handle={sides || handle.length === 2 || handle === 'n' || handle === 's' ? handle : cornerOf(handle)}
      aria-hidden
      className={cn('gizmo-handle', `is-${handle}`, compact && 'is-out')}
    />
  ));
}

/** A picture's left or right edge keeps its shape, as its nearer bottom corner does. */
function cornerOf(handle: Handle): Handle {
  return handle === 'w' ? 'sw' : 'se';
}
