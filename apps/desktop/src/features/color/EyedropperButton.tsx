import { useLingui } from '@lingui/react/macro';
import { Pipette } from 'lucide-react';
import { useEffect, useId, useRef, useState, type JSX, type RefObject } from 'react';
import { screenColor } from '../../bridge/screenColor';
import { IconButton } from '../../design/IconButton';
import { useStableCallback } from '../../lib/useStableCallback';

/**
 * The eyedropper beside the popover's hex field: a press shows the system's loupe, and the colour the
 * person clicks anywhere on the screen is one pick (`onPick`: the popover's custom colour, so it keeps the setting's
 * opacity and goes to Recent). Escape cancels quietly. While the loupe shows the button is inert (`aria-disabled`, so
 * it keeps focus), a second press does nothing, and Escape is the loupe's: the popover stays open (it reads `running`).
 * A failure says so on the status line, which also describes the button, and the popover and focus stay. Shown only
 * where the computer has an eyedropper (a Mac).
 *
 * The status line takes a row of its own: it sits in the hex field's wrapping row (`ColorPopover`).
 */
export function EyedropperButton({
  onPick,
  running,
}: {
  onPick(hex: string): void;
  /**
   * A pick is running: set at the press itself, so a second press in the same frame is ignored too, and cleared when
   * it answers. The popover's, so its Escape waits too.
   */
  running: RefObject<boolean>;
}): JSX.Element | null {
  const { t } = useLingui();
  const [picking, setPicking] = useState(false);
  const [failed, setFailed] = useState(false);
  /** A pick answered after the popover closed changes nothing: the field it was for may be gone. */
  const mounted = useRef(false);
  const pick = useStableCallback(onPick);
  const statusId = useId();
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  if (!screenColor.supported()) return null;

  const press = async () => {
    if (running.current) return;
    running.current = true;
    setPicking(true);
    setFailed(false);
    try {
      const hex = await screenColor.pick();
      if (hex !== null && mounted.current) pick(hex);
    } catch {
      if (mounted.current) setFailed(true);
    } finally {
      running.current = false;
      if (mounted.current) setPicking(false);
    }
  };

  return (
    <>
      <span className="eyedropper">
        <IconButton
          icon={Pipette}
          label={t({
            id: 'color.eyedropper',
            message: 'Pick a color from the screen',
            comment: 'The eyedropper beside the hex field: pick a colour anywhere on the screen',
          })}
          className="icon-btn-sm"
          aria-disabled={picking || undefined}
          aria-describedby={statusId}
          // No tooltip on keyboard focus: it would take the Escape that closes the popover.
          onFocus={(e) => e.preventDefault()}
          onClick={() => void press()}
        />
      </span>
      <p id={statusId} role="status" className="field-note eyedropper-status">
        {failed
          ? t({
              id: 'color.eyedropper.failed',
              message: 'Couldn’t pick a color from the screen. Try again.',
              comment: 'Said under the hex field when the eyedropper failed to start or answer',
            })
          : ''}
      </p>
    </>
  );
}
