import { useLingui } from '@lingui/react/macro';
import type { Theme } from '../bindings/Theme';
import { Segmented } from '../design/Segmented';
import { useUi } from '../state/ui';

/**
 * Appearance in Settings: System (the system's appearance, the default), Light or Dark. Applied at once and kept on this
 * Mac (`useThemeSync`).
 */
export function AppearanceSettings() {
  const { t } = useLingui();
  const theme = useUi((s) => s.theme);
  const setTheme = useUi((s) => s.setTheme);
  const title = t({ id: 'settings.appearance', message: 'Appearance' });
  return (
    <section className="appearance-settings" aria-labelledby="settings-appearance-title">
      <h3 className="section-title" id="settings-appearance-title">
        {title}
      </h3>
      <Segmented<Theme>
        label={title}
        value={theme}
        onChange={setTheme}
        options={[
          {
            value: 'system',
            label: t({
              id: 'settings.appearance.system',
              message: 'System',
              comment: 'Follow the system theme',
            }),
          },
          { value: 'light', label: t({ id: 'settings.appearance.light', message: 'Light' }) },
          { value: 'dark', label: t({ id: 'settings.appearance.dark', message: 'Dark' }) },
        ]}
      />
    </section>
  );
}
