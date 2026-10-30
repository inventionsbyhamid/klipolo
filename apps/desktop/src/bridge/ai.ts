import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { AiStatus } from '../bindings/AiStatus';
import { call } from './call';

/** What `onStatus` gives back to stop listening (so features never import Tauri themselves). */
export type { UnlistenFn };

/**
 * Settings' commands (`crates/klipolo-app/src/commands/settings.rs`): how Klipolo pays for Meta's models. A failure is
 * an Error with one plain sentence. The key and the code go to Rust and are never kept on the page.
 */
export const aiBridge = {
  /** The mode, and the access code's credit and spend (Rust asks the relay). */
  status: () => call<AiStatus>('settings_get'),
  /** Checks the key with Meta and saves it in the Keychain only when it works. */
  setKey: (key: string) => call<AiStatus>('settings_set_key', { key }),
  /** Registers this Mac with the access code. */
  setCode: (code: string) => call<AiStatus>('settings_set_code', { code }),
  /** Removes the key and the code. */
  clear: () => call<AiStatus>('settings_clear'),
  /** Every change to the AI status, whoever made it. */
  onStatus: (handler: (status: AiStatus) => void): Promise<UnlistenFn> =>
    listen<AiStatus>('ai-status', (event) => handler(event.payload)),
};
