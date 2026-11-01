import { useId } from 'react';
import { Button } from '../../design/Button';
import { useExport } from '../../state/export';

/** "Slower. Redoes 3 finished parts.": what finishing in compatibility mode costs. */
export function redoText(parts: number): string {
  return parts === 1 ? 'Slower. Redoes 1 finished part.' : `Slower. Redoes ${parts} finished parts.`;
}

/**
 * "Finish in compatibility mode": offered when the hardware path kept failing. Slower, and it redoes the finished parts
 * the hardware encoder made (none: no second line).
 */
export function CompatibilityButton({ jobId, partsToRedo }: { jobId: string; partsToRedo: number }) {
  const id = useId();
  return (
    <span className="ex-compat">
      <Button
        size="sm"
        aria-describedby={partsToRedo > 0 ? id : undefined}
        onClick={() => void useExport.getState().resume(jobId, 'compatible')}
      >
        Finish in compatibility mode
      </Button>
      {partsToRedo > 0 && (
        <span id={id} className="ex-meta">
          {redoText(partsToRedo)}
        </span>
      )}
    </span>
  );
}
