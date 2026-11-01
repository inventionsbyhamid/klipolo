import { useLingui } from '@lingui/react/macro';
import { useShallow } from 'zustand/react/shallow';
import { useFonts } from '../../state/fonts';
import { DownloadLine } from './DownloadLine';

/**
 * The lines under the font menu's list, always in view and read with it: "Google Fonts is off", each download's
 * progress with its Cancel (its row shows a bar too), and why a download didn't happen, said politely.
 */
export function FontMenuNotes({ offId, error }: { offId: string; error: string | null }) {
  const { t } = useLingui();
  const google = useFonts((s) => s.googleFonts);
  const downloading = useFonts(useShallow((s) => Object.keys(s.downloads)));
  return (
    <>
      {!google && (
        <p id={offId} className="font-note">
          {t({ id: 'fontMenu.googleOff', message: 'Google Fonts is off. Turn it on in Settings.' })}
        </p>
      )}
      {downloading.map((family) => (
        <DownloadLine key={family} family={family} />
      ))}
      <p aria-live="polite" className="font-note font-error">
        {error}
      </p>
    </>
  );
}
