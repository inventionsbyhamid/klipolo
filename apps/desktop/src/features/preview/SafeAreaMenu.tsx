import { useLingui } from '@lingui/react/macro';
import { Menu, MenuContent, MenuRadioGroup, MenuRadioItem, MenuTrigger } from '../../design/Menu';
import { Icon } from '../../layout/Icon';
import { SAFE_AREA_IDS, type SafeAreaId } from '../../lib/safeAreas';
import { useUi } from '../../state/ui';
import { useShownFormat } from '../editor/formats';

/**
 * The 9:16 safe area, in the preview's header: off, every platform's combined area, or one app's, drawn over the video
 * (`SafeArea`). Its menu is there while the preview shows 9:16; elsewhere the button is off and says where it shows,
 * and the choice waits for 9:16. Kept on this Mac; never in the project or an export.
 */
export function SafeAreaMenu() {
  const { t } = useLingui();
  const portrait = useShownFormat() === 'portrait';
  const area = useUi((s) => s.safeArea);
  const setSafeArea = useUi((s) => s.setSafeArea);
  if (!portrait)
    return (
      // Focusable, so its tooltip says why it's off.
      <button
        type="button"
        className="icon-btn"
        aria-label={t({ id: 'safeArea.label', message: 'Safe area' })}
        title={t({ id: 'preview.safeArea.off', message: 'Shows in 9:16' })}
        aria-disabled
      >
        <Icon name="safe" />
      </button>
    );
  const names: Record<SafeAreaId, string> = {
    off: t({
      id: 'safeArea.off',
      message: 'Off',
      comment: 'Safe area choice: nothing drawn over the 9:16 preview (a drag still shows the guide)',
    }),
    all: t({
      id: 'safeArea.all',
      message: 'All platforms',
      comment: 'Safe area choice: the area TikTok, Instagram Reels and YouTube Shorts all leave clear',
    }),
    tiktok: t({ id: 'safeArea.tiktok', message: 'TikTok', comment: "The app's name (safe area choice)" }),
    reels: t({ id: 'safeArea.reels', message: 'Instagram Reels', comment: "The app's name (safe area choice)" }),
    shorts: t({ id: 'safeArea.shorts', message: 'YouTube Shorts', comment: "The app's name (safe area choice)" }),
  };
  const name = names[area];
  return (
    <Menu>
      <MenuTrigger asChild>
        <button
          type="button"
          className="icon-btn"
          aria-label={t({
            id: 'safeArea.menu',
            message: `Safe area: ${name}`,
            comment: 'The safe area menu button; {name} is the choice shown, e.g. Off or TikTok',
          })}
          title={t({
            id: 'safeArea.tooltip',
            message: `Safe area: ${name}. Show where each app's buttons cover the video.`,
            comment: 'Tooltip of the safe area menu button: the choice shown ({name}), then what the menu does',
          })}
          data-on={area !== 'off' || undefined}
        >
          <Icon name="safe" />
        </button>
      </MenuTrigger>
      <MenuContent align="end">
        <MenuRadioGroup<SafeAreaId> value={area} onValueChange={setSafeArea}>
          {SAFE_AREA_IDS.map((id) => (
            <MenuRadioItem key={id} value={id}>
              {names[id]}
            </MenuRadioItem>
          ))}
        </MenuRadioGroup>
      </MenuContent>
    </Menu>
  );
}
