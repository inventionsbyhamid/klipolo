import { i18n, type I18n, type MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import type { ContribDef } from '../bindings/ContribDef';
import type { ContribId } from '../bindings/ContribId';
import type { ContribKind } from '../bindings/ContribKind';
import { agentName } from './agentNames';
import catalog from './builtin-catalog.json';
import { contribName } from './contribNames';

/**
 * Built-in contributions, pinned to Rust by `crates/klipolo-model/tests/contrib.rs` (`builtin_catalog_json_is_current`).
 * Their `name` is English from Rust; the UI shows `contribLabel`, which names a built-in by its id in the language
 * shown (`contribNames`).
 */
export const BUILTINS = catalog as ContribDef[];

export function builtinId(local: string): ContribId {
  return `klipolo.builtin/${local}`;
}

export function builtinsOf(kind: ContribKind): ContribDef[] {
  return BUILTINS.filter((def) => def.kind === kind);
}

/** The name a layer or setting shows when its contribution isn't installed. */
const MISSING: Record<ContribKind, MessageDescriptor> = {
  look: msg({ id: 'contrib.missing.look', message: 'Missing look' }),
  motion: msg({ id: 'contrib.missing.motion', message: 'Missing animation' }),
  audioEffect: msg({ id: 'contrib.missing.audioEffect', message: 'Missing audio effect' }),
  effect: msg({ id: 'contrib.missing.effect', message: 'Missing effect' }),
  generator: msg({ id: 'contrib.missing.generator', message: 'Missing generator' }),
  transition: msg({ id: 'contrib.missing.transition', message: 'Missing transition' }),
  captionStyle: msg({ id: 'contrib.missing.captionStyle', message: 'Missing caption style' }),
  scene: msg({
    id: 'contrib.missing.scene',
    message: 'Missing motion graphic',
    comment: 'A motion graphic (an animated title or graphic made as a scene) this version lacks',
  }),
};

export function isInstalled(id: ContribId): boolean {
  return BUILTINS.some((def) => def.id === id);
}

/**
 * A motion graphic (`scene@1`): its local part starts with `scene.` (`klipolo.builtin/scene.<name>`,
 * `lib.<id>/scene.<slug>.v<N>`), as the model's `ContribId::is_scene` says; never judged by a layer's name. A
 * generator layer's source may be one.
 */
export function isScene(id: ContribId): boolean {
  const slash = id.indexOf('/');
  return slash >= 0 && id.startsWith('scene.', slash + 1);
}

/**
 * A contribution's name in the language shown (or `inI18n`'s); one that isn't installed reads "Missing look", "Missing
 * effect"… (show its id as a tooltip).
 */
export function contribLabel(id: ContribId, kind: ContribKind, inI18n: I18n = i18n): string {
  return (
    contribName(id, inI18n) ??
    BUILTINS.find((def) => def.id === id)?.name ??
    agentName(id) ??
    inI18n._(MISSING[kind])
  );
}

/** The same, inside a sentence ("Remove missing effect"): its own message, so each language cases it its own way. */
const MISSING_IN_SENTENCE: Record<ContribKind, MessageDescriptor> = {
  look: msg({
    id: 'contrib.missingInSentence.look',
    message: 'missing look',
    comment: 'A look this version lacks, inside a sentence: “Apply missing look”',
  }),
  motion: msg({
    id: 'contrib.missingInSentence.motion',
    message: 'missing animation',
    comment: 'An animation (a layer’s motion preset) this version lacks, inside a sentence',
  }),
  audioEffect: msg({
    id: 'contrib.missingInSentence.audioEffect',
    message: 'missing audio effect',
    comment: 'A sound effect this version lacks, inside a sentence',
  }),
  effect: msg({
    id: 'contrib.missingInSentence.effect',
    message: 'missing effect',
    comment: 'An effect this version lacks, inside a sentence: “Remove missing effect”',
  }),
  generator: msg({
    id: 'contrib.missingInSentence.generator',
    message: 'missing generator',
    comment: 'A generator this version lacks, inside a sentence',
  }),
  transition: msg({
    id: 'contrib.missingInSentence.transition',
    message: 'missing transition',
    comment: 'A transition this version lacks, inside a sentence',
  }),
  captionStyle: msg({
    id: 'contrib.missingInSentence.captionStyle',
    message: 'missing caption style',
    comment: 'A caption style this version lacks (a style pack not installed), inside a sentence',
  }),
  scene: msg({
    id: 'contrib.missingInSentence.scene',
    message: 'missing motion graphic',
    comment: 'A motion graphic this version lacks, inside a sentence: “Remove missing motion graphic”',
  }),
};

/** `contribLabel` for a name inside a sentence (an undo label's value): one that isn't installed is "missing effect". */
export function contribLabelInSentence(id: ContribId, kind: ContribKind, inI18n: I18n = i18n): string {
  return (
    contribName(id, inI18n) ??
    BUILTINS.find((def) => def.id === id)?.name ??
    agentName(id) ??
    inI18n._(MISSING_IN_SENTENCE[kind])
  );
}
