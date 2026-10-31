import type { MessageDescriptor } from '@lingui/core';
import { msg, plural, t } from '@lingui/core/macro';
import type { ExportQuality } from '../../bindings/ExportQuality';
import type { FormatKind } from '../../bindings/FormatKind';
import { FORMAT_SIZE } from '../../lib/layout';

/** The formats an export can make, one file each, in the order the dialog shows them. */
export const FORMATS: FormatKind[] = ['landscape', 'portrait', 'square'];

/** A format's pixel size: the plan's (16:9 follows the sources up to 4K), else its usual one. */
export function formatSize(format: FormatKind, size?: { width: number; height: number }): [number, number] {
  return size ? [size.width, size.height] : FORMAT_SIZE[format];
}

export interface QualityOption {
  value: ExportQuality;
  label: MessageDescriptor;
}

/** One quality for every file of the export: High, or a smaller file that uploads well. */
export const QUALITIES: QualityOption[] = [
  { value: 'standard', label: msg({ id: 'export.quality.standard', message: 'Smaller file' }) },
  { value: 'high', label: msg({ id: 'export.quality.high', message: 'High' }) },
];

/** "1 video", "2 videos". */
export function videos(count: number): string {
  return t({ id: 'export.videos', message: plural(count, { one: '# video', other: '# videos' }) });
}

/** The measured time left: "About 15 seconds left", "About 3 minutes left". */
export function timeLeft(seconds: number): string {
  if (seconds < 55) {
    const shown = Math.max(5, Math.ceil(seconds / 5) * 5);
    return t({
      id: 'export.left.seconds',
      message: plural(shown, { one: 'About # second left', other: 'About # seconds left' }),
    });
  }
  if (seconds < 90) return t({ id: 'export.left.aboutMinute', message: 'About a minute left' });
  if (seconds < 5400) {
    const minutes = Math.round(seconds / 60);
    return t({
      id: 'export.left.minutes',
      message: plural(minutes, { one: 'About # minute left', other: 'About # minutes left' }),
    });
  }
  const hours = Math.round(seconds / 3600);
  return t({
    id: 'export.left.hours',
    message: plural(hours, { one: 'About # hour left', other: 'About # hours left' }),
  });
}
