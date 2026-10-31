import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react/macro';
import { useId } from 'react';
import { Toggle } from '../../design/Toggle';
import { sayRefusal, useMediaJobs } from '../../state/mediaJobs';
import { useUi } from '../../state/ui';

/** The popover's heading. */
export const COPIES_TITLE = msg({ id: 'mediaJobs.title', message: 'Editing copies' });

/** What editing copies are, under the popover's heading. */
export const ABOUT_COPIES = msg({
  id: 'mediaJobs.about',
  message:
    'Klipolo makes lighter copies of large videos so the preview plays smoothly. Exports always use your full-quality files.',
});

/** "Make editing copies automatically": the popover's switch (kept on this Mac). */
export function AutoCopiesSwitch() {
  const { t } = useLingui();
  const id = useId();
  const on = useUi((s) => s.autoCopies);
  return (
    <div className="copies-auto">
      <label htmlFor={id}>{t({ id: 'mediaJobs.auto', message: 'Make editing copies automatically' })}</label>
      <Toggle
        id={id}
        checked={on}
        onCheckedChange={(next) => void useMediaJobs.getState().setAuto(next).then(sayRefusal)}
      />
    </div>
  );
}
