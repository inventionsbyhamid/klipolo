import type { PointerEvent } from 'react';
import type { Edge } from '../../../bindings/Edge';

const EDGES = ['start', 'end'] as const;

interface TrimHandlesProps {
  selected: boolean;
  onTrimPress: (e: PointerEvent<HTMLElement>, edge: Edge) => void;
}

/**
 * A block's two edges: drag one to trim it. Each is a 12 px pad (at most a quarter of the block, so a narrow one still
 * moves) over the labels; a selected block shows its grips there (paper on clips, on-paper ink on a
 * selected overlay). Pointer only, hidden from screen readers: the keyboard trims with Q and W.
 */
export function TrimHandles({ selected, onTrimPress }: TrimHandlesProps) {
  return EDGES.map((edge) => (
    <span
      key={edge}
      aria-hidden
      data-trim={edge}
      data-shown={selected || undefined}
      onPointerDown={(e) => onTrimPress(e, edge)}
      className="tl-trim"
    />
  ));
}
