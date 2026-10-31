import type { FormatKind } from '../../bindings/FormatKind';
import type { Op } from '../../bindings/Op';
import { FORMAT_SHORT } from '../../lib/layout';
import { commitLatest } from '../../state/commit';
import { useProject } from '../../state/project';
import { useUi } from '../../state/ui';

/** The formats in the order the pills, the 1, 2, 3 keys and the project list them. */
export const FORMAT_ORDER: FormatKind[] = ['landscape', 'portrait', 'square'];

/** The format the preview shows: the one picked for the preview, else the project's main one. */
export function useShownFormat(): FormatKind {
  const picked = useUi((s) => s.previewFormat);
  const primary = useProject((s) => s.snapshot?.project.settings.primaryFormat ?? 'landscape');
  return picked ?? primary;
}

/**
 * Makes `format` the project's main format, as one undo step ("Switch to 9:16"): one `setSettings` op, every format the
 * project has kept and `format` joining them. The model keeps each format's hand override and how the format left looks;
 * a format without an override of its own (1:1, say) re-derives its layers' places from the new primary, so they can
 * move there. Built at its turn in the commit queue from the settings then:
 * two quick switches each build on the one before. The format already shown changes nothing. Resolves with whether it
 * went in (true when there was nothing to do).
 */
export async function switchFormat(format: FormatKind): Promise<boolean> {
  const landed = await commitLatest({ id: 'format.switch.undo', params: { ratio: FORMAT_SHORT[format] } }, (): Op[] => {
    const settings = useProject.getState().snapshot?.project.settings;
    if (!settings || settings.primaryFormat === format) return [];
    const formats = FORMAT_ORDER.filter((f) => f === format || settings.formats.includes(f));
    return [{ type: 'setSettings', settings: { ...settings, formats, primaryFormat: format } }];
  });
  // The preview follows the main format from now on (a format picked only for the preview no longer holds).
  if (landed) useUi.getState().setPreviewFormat(null);
  return landed;
}
