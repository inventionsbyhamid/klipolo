import type { ClipFraming } from '../bindings/ClipFraming';
import { call } from './call';

/**
 * Clip `clipId`'s framing in every format as the engine draws it (its own, else the model's default) and whether it
 * can follow the cursor (`crates/klipolo-app/src/commands/framing.rs`). Fails for a clip that's gone or with no
 * project open.
 */
export const clipFraming = (clipId: string) => call<ClipFraming>('clip_framing', { clipId });
