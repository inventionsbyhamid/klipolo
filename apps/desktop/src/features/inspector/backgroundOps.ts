import type { Background } from '../../bindings/Background';
import type { BackgroundKind } from '../../bindings/BackgroundKind';
import type { Op } from '../../bindings/Op';
import { useProject } from '../../state/project';

/** The project's background as it is now (when a change is built), or null with no project open. */
export function backgroundNow(): Background | null {
  return useProject.getState().snapshot?.project.background ?? null;
}

/**
 * `setBackground` with `patch` over the background as the project has it when the change is built (`setBackground`
 * carries it whole, so a colour change still on its way keeps a padding drag's value); none with no project open.
 */
export function backgroundOps(patch: Partial<Background> | ((now: Background) => Partial<Background>)): Op[] {
  const now = backgroundNow();
  if (!now) return [];
  const change = typeof patch === 'function' ? patch(now) : patch;
  return [{ type: 'setBackground', background: { ...now, ...change } }];
}

/** The second colour a gradient starts with when it has none yet (the swatches' blue). */
export const GRADIENT_END = '#7F92DA';

/**
 * What choosing `kind` sets on `now`: a gradient gets a second colour when it has none, a picture the project's first
 * image (`firstImage`) when it names none.
 */
export function kindPatch(now: Background, kind: BackgroundKind, firstImage: string | null): Partial<Background> {
  switch (kind) {
    case 'gradient':
      return { kind, colorEnd: now.colorEnd ?? GRADIENT_END };
    case 'image':
      return { kind, assetId: now.assetId ?? firstImage };
    default:
      return { kind };
  }
}
