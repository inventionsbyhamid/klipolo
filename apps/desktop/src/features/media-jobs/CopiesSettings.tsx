import { useLingui } from '@lingui/react/macro';
import { ABOUT_COPIES, AutoCopiesSwitch, COPIES_TITLE } from './AutoCopiesSwitch';
import './mediaJobs.css';

/**
 * Editing copies in Settings: the same switch as the top bar's pill, one setting behind both. Found here while no copy
 * is being made (the pill shows only then), so copies turned off can be turned on again.
 */
export function CopiesSettings() {
  const { t } = useLingui();
  return (
    <section className="copies-settings" aria-labelledby="settings-copies-title">
      <h3 className="section-title" id="settings-copies-title">
        {t(COPIES_TITLE)}
      </h3>
      <AutoCopiesSwitch />
      <p className="hint">{t(ABOUT_COPIES)}</p>
    </section>
  );
}
