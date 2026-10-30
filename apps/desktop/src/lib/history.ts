import { i18n } from '@lingui/core';
import { t } from '@lingui/core/macro';
import type { ErrorParam } from '../bindings/ErrorParam';
import { wordLabel } from './labels';

/**
 * The Undo or Redo control's name, with the history label when there is one: "Undo add text" from `text.add`. The
 * label is a message id with params, worded in the language shown now, so a language switch rewords it.
 *
 * In English the label reads as the rest of the sentence, so its first letter is lowercased unless the first word is
 * an acronym or name in capitals ("LUT", "TikTok" stay as they are). Other languages keep the label's own casing.
 */
export function historyAction(
  kind: 'undo' | 'redo',
  id: string | null,
  params?: Record<string, ErrorParam> | null,
): string {
  const label = id ? wordLabel(id, params) : '';
  if (!label) {
    return kind === 'undo'
      ? t({ id: 'history.undo', message: 'Undo' })
      : t({ id: 'history.redo', message: 'Redo' });
  }
  const action =
    i18n.locale === 'en' && /^[A-Z][a-z]*\b/.test(label)
      ? label.charAt(0).toLowerCase() + label.slice(1)
      : label;
  return kind === 'undo'
    ? t({
        id: 'history.undo.named',
        message: `Undo ${action}`,
        comment: '{action} is the name of the edit to take back, e.g. "add text", "split clip"',
      })
    : t({
        id: 'history.redo.named',
        message: `Redo ${action}`,
        comment: '{action} is the name of the edit to make again, e.g. "add text", "split clip"',
      });
}
