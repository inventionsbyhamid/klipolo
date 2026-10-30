import type { UnlistenFn } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { AiStatus } from '../bindings/AiStatus';
import { aiBridge } from '../bridge/ai';

interface AiStore {
  /** How AI is paid for, as Rust last said; null until the first answer. */
  status: AiStatus | null;
  /** Whether the Settings dialog is open. */
  settingsOpen: boolean;
}

/**
 * The AI status every AI surface reads (the chat panel, the analysis banner, the top bar's chip, Settings). Rust tells
 * every change (`ai-status`), so a surface never asks on its own.
 */
export const useAi = create<AiStore>(() => ({ status: null, settingsOpen: false }));

/** Whether an AI feature has nothing to pay with: no key and no code. Unknown (before the first answer) is not. */
export function needsKey(status: AiStatus | null): boolean {
  return status?.mode === 'none';
}

/** Whether AI features should ask for a key or code right now. */
export function useNeedsKey(): boolean {
  return useAi((s) => needsKey(s.status));
}

export function setAiStatus(status: AiStatus) {
  useAi.setState({ status });
}

/** Asks Rust for the status again (with an access code, that asks the relay for the credit). */
export async function refreshAiStatus(): Promise<AiStatus | null> {
  try {
    const status = await aiBridge.status();
    setAiStatus(status);
    return status;
  } catch {
    return useAi.getState().status;
  }
}

/** Reads the status once and follows every change after it. */
export async function followAiStatus(): Promise<UnlistenFn> {
  const unlisten = await aiBridge.onStatus(setAiStatus);
  await refreshAiStatus();
  return unlisten;
}

export function openSettings() {
  useAi.setState({ settingsOpen: true });
}

export function closeSettings() {
  useAi.setState({ settingsOpen: false });
}
