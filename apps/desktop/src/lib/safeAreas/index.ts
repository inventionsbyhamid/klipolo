import type { BoxN } from '../../bindings/BoxN';
import type { FormatKind } from '../../bindings/FormatKind';
import { ALL } from './all';
import { REELS } from './reels';
import { SHORTS } from './shorts';
import type { SafeAreaId, SafeAreaSpec } from './spec';
import { TIKTOK } from './tiktok';

export { clearFrom, type SafeAreaId, type SafeAreaSpec, type SafeShape } from './spec';

/**
 * Each 9:16 area by its word, in the menu's order: one data file per platform, registered here
 * once. A word `SafeAreaId` gains doesn't compile until it has its file. A view of the preview
 * only: never in the project, never exported.
 */
const BY_ID = {
  all: ALL,
  tiktok: TIKTOK,
  reels: REELS,
  shorts: SHORTS,
} satisfies Record<Exclude<SafeAreaId, 'off'>, SafeAreaSpec>;

export const SAFE_AREAS: SafeAreaSpec[] = Object.values(BY_ID);
/** The menu's choices: off, then every area. */
export const SAFE_AREA_IDS: readonly SafeAreaId[] = ['off', ...SAFE_AREAS.map((s) => s.id)];

export function safeArea(id: SafeAreaId): SafeAreaSpec | null {
  return id === 'off' ? null : BY_ID[id];
}

/** Where nothing of the app's own UI covers the video, or null when it's off or the format has none (only 9:16). */
export function safeRect(format: FormatKind, id: SafeAreaId): BoxN | null {
  return format === 'portrait' ? (safeArea(id)?.clear ?? null) : null;
}

/** What a drag in 9:16 shows and snaps to: the chosen area, else the last one chosen, else all platforms. */
export function dragArea(id: SafeAreaId, last: SafeAreaId): Exclude<SafeAreaId, 'off'> {
  if (id !== 'off') return id;
  return last !== 'off' ? last : 'all';
}
