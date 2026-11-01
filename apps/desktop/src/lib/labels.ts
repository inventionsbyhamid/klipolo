import { i18n, type I18n, type MessageDescriptor } from '@lingui/core';
import type { ErrorParam } from '../bindings/ErrorParam';
import type { ContribKind } from '../bindings/ContribKind';
import { intlLocale, type UiLocale } from '../i18n';
import { CHAT_LABELS } from './chatLabels';
import { contribLabelInSentence } from './contrib';
import { contribName } from './contribNames';
import { formatFileSize, formatSeconds } from './format';
import { FROM_CLIENT, MCP_LABELS, TABLE } from './labelTable';
import { CAPTION_LOOKS } from './textLook';
import { formatTime } from './time';

/**
 * Every undo label the UI makes, by message id: `useProject.apply('text.add', ops)` stores the id, and the Edit menu,
 * ⌘K and the Undo chip word it here in the language shown, so a language switch rewords them at once and chat's
 * English-only planner reads the history in English. Chat's labels (`chat.<tool>`), other apps' (`mcp.*`) and the
 * caption styles' come from their own tables (`addLabelSource`). The UI's ids never use a namespace Rust's codes use
 * (`edit`, `chat`, `model`, `project`…): a test holds them apart, so one id is never two messages.
 */
export const LABELS: Record<string, MessageDescriptor> = TABLE;

/** An undo label's message id: one the UI applies (`LABELS`). */
export type LabelId = keyof typeof TABLE;

/** The values an undo label's message shows (`{ effect: 'klipolo.builtin/effect.blur' }`, `{ ratio: '9:16' }`). */
export type LabelParams = Record<string, ErrorParam>;

/** An undo label as edits pass it along: its id, with its values when its message shows some. */
export type Label = LabelId | { id: LabelId; params: LabelParams };

/** `label`'s id and values, as `useProject.apply` takes them. */
export function labelParts(label: Label): [LabelId, LabelParams | undefined] {
  return typeof label === 'string' ? [label, undefined] : [label.id, label.params];
}

/** Labels from elsewhere: tables of their own, asked after the UI's. */
const extra: Array<(id: string) => MessageDescriptor | undefined> = [];

export function addLabelSource(source: (id: string) => MessageDescriptor | undefined) {
  extra.push(source);
}

// Caption styles, which a label names by their own id as a nested code (`{ style: { code: 'text.look.box' } }`).
const captionStyles = new Map(CAPTION_LOOKS.map((look) => [look.name.id, look.name]));
addLabelSource((id) => captionStyles.get(id));
// Chat's changes (`chat.<tool>`).
addLabelSource((id) => (Object.hasOwn(CHAT_LABELS, id) ? CHAT_LABELS[id] : undefined));
// Other apps' folded steps (`mcp.*`).
addLabelSource((id) => (Object.hasOwn(MCP_LABELS, id) ? MCP_LABELS[id] : undefined));

export function labelDescriptor(id: string): MessageDescriptor | undefined {
  // Own keys only: an id from elsewhere (a project file, chat) is never a property of the table's prototype.
  if (Object.hasOwn(LABELS, id)) return LABELS[id];
  for (const source of extra) {
    const found = source(id);
    if (found) return found;
  }
  return undefined;
}

/**
 * Params named after a contribution kind hold its id (`{ effect: … }`, `{ look: … }`): one this build lacks reads
 * "missing effect" (in a sentence: "Remove missing effect"), never its id.
 */
const CONTRIB_PARAMS: Record<string, ContribKind> = { effect: 'effect', look: 'look' };

/**
 * A spoken language's code (a `language` value: "de") as its name in the language shown ("German"); the code itself
 * when it isn't one.
 */
function languageName(code: string, inI18n: I18n): string {
  try {
    return (
      new Intl.DisplayNames([intlLocale(inI18n.locale as UiLocale)], { type: 'language' }).of(code) ?? code
    );
  } catch {
    return code;
  }
}

/** A value's words by its name, as Rust's `english()` words them (`klipolo_model::coded`). */
function wordNumber(key: string, value: number, inI18n: I18n): string {
  // A short length ("2.0 s"); a bare `durationUs` is a video's length and reads as a clock.
  if (key.endsWith('DurationUs')) return formatSeconds(intlLocale(inI18n.locale as UiLocale), value / 1e6);
  // A file size ("190 MB"), as Rust's `english` writes it too.
  if (key.endsWith('Bytes')) return formatFileSize(intlLocale(inI18n.locale as UiLocale), value);
  // A moment (`…AtMs`, ms since 1970) in the person's time zone and language.
  if (key.endsWith('AtMs'))
    return new Intl.DateTimeFormat(intlLocale(inI18n.locale as UiLocale), {
      dateStyle: 'medium',
      timeStyle: 'short',
    }).format(value);
  return key.endsWith('Us') ? formatTime(value) : inI18n.number(value);
}

/**
 * A label's values in words: a time (`…Us`) as a clock, a short length (`…DurationUs`) in seconds, a moment (`…AtMs`)
 * as a date and time, a size (`…Bytes`) as a file size, a contribution id as its name, a nested code worded the same
 * way; other text as it is (a clip's name). `inI18n` is the catalogue to word with: the language shown, or the
 * built-in English for the planner.
 */
export function wordParams(
  params: LabelParams | null | undefined,
  inI18n: I18n = i18n,
): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(params ?? {})) {
    if (typeof value === 'number') out[key] = wordNumber(key, value, inI18n);
    else if (typeof value === 'string')
      out[key] = Object.hasOwn(CONTRIB_PARAMS, key)
        ? contribLabelInSentence(value, CONTRIB_PARAMS[key], inI18n)
        : key === 'language'
          ? languageName(value, inI18n)
          : (contribName(value, inI18n) ?? value);
    else out[key] = wordLabel(value.code, value.params, inI18n);
  }
  return out;
}

/**
 * `id` with `params`, in the language shown (or `inI18n`'s); '' for an id nothing knows. A step another app made (its
 * params name the app as `client`) says so: "Make it vertical (from Example Client)"; its folded step names the app already.
 */
export function wordLabel(id: string, params?: LabelParams | null, inI18n: I18n = i18n): string {
  const descriptor = labelDescriptor(id);
  if (!descriptor) return '';
  // By id, so the label's own values are the ones worded (a descriptor's would win over them).
  const label = inI18n._(descriptor.id, wordParams(params, inI18n), { message: descriptor.message });
  const client = params?.client;
  if (typeof client !== 'string' || Object.hasOwn(MCP_LABELS, id)) return label;
  return inI18n._(FROM_CLIENT.id, { label, client }, { message: FROM_CLIENT.message });
}

/** `label` in the language shown: a command's or a button's name that is also its undo label. */
export function labelText(label: Label): string {
  const [id, params] = labelParts(label);
  return wordLabel(id, params);
}
