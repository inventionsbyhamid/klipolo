import type { ContribDef } from '../../bindings/ContribDef';
import type { ParamSpec } from '../../bindings/ParamSpec';
import { valueOf, type ParamValues } from './paramFormat';

/**
 * Whether setting `spec` applies given the others, from its schema's `visibleWhen` alone (no per-effect code): while
 * the choice it names holds one of the listed values, and that choice applies itself (a chain of conditions hides
 * everything below a hidden choice). A setting without `visibleWhen` always applies, and so does one whose condition
 * names a setting the schema lacks (the engine refuses such a definition when it loads).
 *
 * Purely presentational: a hidden setting keeps its stored value, which counts again when the choice comes back.
 * O(depth x settings) a call, a handful of each; a cycle (which the engine refuses too) stops after one pass.
 */
export function isShown(def: ContribDef, spec: ParamSpec, values: ParamValues, depth = 0): boolean {
  const when = spec.visibleWhen;
  if (!when || depth > def.params.length) return true;
  const on = def.params.find((p) => p.key === when.key);
  if (!on) return true;
  const now = valueOf(on, values);
  return typeof now === 'string' && when.in.includes(now) && isShown(def, on, values, depth + 1);
}
