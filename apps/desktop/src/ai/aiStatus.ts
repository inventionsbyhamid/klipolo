import { create } from 'zustand';
import type { AiStatus } from '../bindings/AiStatus';
import { aiBridge, type UnlistenFn } from '../bridge/ai';

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

/** What the free credit says once it is used up (Rust says the same in the status's notice). */
const FREE_CREDIT_USED_UP = 'Your free credit is used up. Add your own Meta key in Settings to keep using AI.';

/** The free credit left in dollars, rounded to the cent, when the free credit pays and the relay said; else null. */
export function freeCreditLeft(status: AiStatus | null): number | null {
  if (!status?.trial || status.budgetUsd == null || status.spentUsd == null) return null;
  return Math.max(0, Math.round((status.budgetUsd - Math.max(0, status.spentUsd)) * 100) / 100);
}

/** Whether the free credit pays and nothing is left of it, or the relay turned it off. */
export function freeCreditUsedUp(status: AiStatus | null): boolean {
  if (!status?.trial) return false;
  const left = freeCreditLeft(status);
  return status.active === false || (left !== null && left <= 0);
}

/**
 * What every AI surface says while the free credit built into Klipolo pays: "Free credit: $4.80 left. No key needed.",
 * that it is used up, or why the credit is unknown (Rust's notice). Null when it doesn't pay (a key, the person's own
 * code, or nothing).
 */
export function freeCreditText(status: AiStatus | null): string | null {
  if (!status?.trial) return null;
  if (freeCreditUsedUp(status)) return FREE_CREDIT_USED_UP;
  if (status.notice) return status.notice;
  const left = freeCreditLeft(status);
  return left === null ? 'Free credit. No key needed.' : `Free credit: $${left.toFixed(2)} left. No key needed.`;
}

/** Whether AI features should ask for a key or code right now. */
export function useNeedsKey(): boolean {
  return useAi((s) => needsKey(s.status));
}

/** Changes told so far (saves and `ai-status` events): a read asked for before the latest one is out of date. */
let changes = 0;

/** A status Rust told or a save answered: always the newest. */
export function setAiStatus(status: AiStatus) {
  changes += 1;
  useAi.setState({ status });
}

/**
 * Asks Rust for the status again (with an access code, that asks the relay for the credit, which can take seconds).
 * An answer is dropped when a save or an `ai-status` event came in while it was asked: it may describe credentials
 * that were removed or replaced since. Gives the status kept.
 */
export async function refreshAiStatus(): Promise<AiStatus | null> {
  const asked = changes;
  try {
    const status = await aiBridge.status();
    if (changes === asked) useAi.setState({ status });
  } catch {
    // The status stays as it was.
  }
  return useAi.getState().status;
}

/** The credit ask under way, and whether another paid job ended while it was asked. */
let asking = false;
let movedAgain = false;

/**
 * A paid job ended, done or failed (analysis, chat, a picture, following the speaker): the credit the top bar and
 * Settings show, and the session's spend, are asked for again. One ask at a time: jobs that end while it is asked make
 * one more ask after it. Nothing to ask with no key and no code.
 */
export function creditMoved() {
  if (useAi.getState().status?.mode === 'none') return;
  if (asking) {
    movedAgain = true;
    return;
  }
  asking = true;
  void refreshAiStatus().finally(() => {
    asking = false;
    if (movedAgain) {
      movedAgain = false;
      creditMoved();
    }
  });
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
