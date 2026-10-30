import type { ContribId } from '../bindings/ContribId';

/**
 * The names of agent-made effects the open project can draw. Klipolo has none, so every id that isn't a built-in reads
 * as missing (`contrib.ts`).
 */
export function agentName(_id: ContribId): string | null {
  return null;
}
