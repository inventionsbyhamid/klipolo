import { i18n, type MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import type { ParamSpec } from '../../bindings/ParamSpec';

/**
 * The built-in effects' and generators' setting names that the app words its own way ("Color", "Center"), keyed by
 * the schema's label. Every other setting shows its schema's label as it is.
 */
const WORDED: Record<string, MessageDescriptor> = {
  Colour: msg({ id: 'param.color', message: 'Color', comment: 'An effect setting' }),
  Centre: msg({ id: 'param.center', message: 'Center', comment: 'An effect setting: where a shape sits' }),
  'Border colour': msg({ id: 'param.borderColor', message: 'Border color', comment: 'An effect setting' }),
  'Sky colour': msg({ id: 'param.skyColor', message: 'Sky color', comment: 'A background setting' }),
  'First colour': msg({ id: 'param.firstColor', message: 'First color', comment: 'A background setting' }),
  'Second colour': msg({ id: 'param.secondColor', message: 'Second color', comment: 'A background setting' }),
  'Third colour': msg({ id: 'param.thirdColor', message: 'Third color', comment: 'A background setting' }),
  'Speck colour': msg({ id: 'param.speckColor', message: 'Speck color', comment: 'A background setting' }),
  'Start colour': msg({ id: 'param.startColor', message: 'Start color', comment: 'A gradient setting' }),
  'End colour': msg({ id: 'param.endColor', message: 'End color', comment: 'A gradient setting' }),
  'Warm colour': msg({ id: 'param.warmColor', message: 'Warm color', comment: 'A light leak setting' }),
  'Soft colour': msg({ id: 'param.softColor', message: 'Soft color', comment: 'A light leak setting' }),
  'Deep colour': msg({ id: 'param.deepColor', message: 'Deep color', comment: 'A waves setting' }),
  'Colour noise': msg({ id: 'param.colorNoise', message: 'Color noise', comment: 'A film grain setting' }),
};

/** A setting's name in the language shown. */
export function settingLabel(spec: ParamSpec): string {
  const worded = WORDED[spec.label];
  return worded ? i18n._(worded) : spec.label;
}
