import type { AiStatus } from '../bindings/AiStatus';
import { setAiStatus } from '../ai/aiStatus';
import { aiBridge } from '../bridge/ai';

/** Which way to pay the dialog shows: an access code or the person's own Meta API key. */
export type PayChoice = 'code' | 'key';

/** The access code's credit as the dialog draws it: spend clamped to the budget, so nothing shows below $0.00. */
export interface BudgetView {
  budget: number;
  spent: number;
  left: number;
  /** What is left as a share of the budget, 0 to 1 (the bar). */
  shareLeft: number;
}

/** What AI cost this session as the dialog says it: the total, and each kind that cost something. */
export interface SessionView {
  total: string;
  kinds: string[];
}

/** "Analysis $0.08", "Chat $0.02 for 4 messages", "Pictures $0.02 for 2"; null before anything was spent. */
export function sessionView(status: AiStatus | null): SessionView | null {
  const s = status?.session;
  if (!s) return null;
  const kinds: string[] = [];
  if (s.analysisUsd > 0) kinds.push(`Analysis ${spend(s.analysisUsd)}`);
  if (s.chatUsd > 0 || s.chatMessages > 0)
    kinds.push(`Chat ${spend(s.chatUsd)} for ${s.chatMessages} ${s.chatMessages === 1 ? 'message' : 'messages'}`);
  if (s.picturesUsd > 0 || s.pictures > 0) kinds.push(`Pictures ${spend(s.picturesUsd)} for ${s.pictures}`);
  if (kinds.length === 0) return null;
  return { total: spend(s.analysisUsd + s.chatUsd + s.picturesUsd), kinds };
}

/** Dollars and cents, or "under $0.01" for a cost that rounds to nothing. */
function spend(amount: number): string {
  return amount > 0 && amount < 0.005 ? 'under $0.01' : usd(amount);
}

/** The top bar's chip: what it says and which icon it shows. */
export interface ChipView {
  icon: 'key' | 'ticket';
  label: string;
  /** The credit left, for an access code whose credit is known. */
  left?: string;
  /** No key and no code: the outlined "Add Meta key" chip. */
  missing: boolean;
}

/** The relay may pass the budget by the last call's cost; the dialog never shows more spent than the budget. */
export function budgetView(status: AiStatus): BudgetView | null {
  const { budgetUsd, spentUsd } = status;
  if (status.mode !== 'relay' || budgetUsd == null || spentUsd == null) return null;
  const budget = Math.max(0, budgetUsd);
  const spent = Math.min(Math.max(0, spentUsd), budget);
  const left = cents(budget - spent);
  return { budget, spent, left, shareLeft: budget > 0 ? cents(left / budget) : 0 };
}

/** Dollars and cents: "$1.94". */
export function usd(amount: number): string {
  return `$${Math.max(0, amount).toFixed(2)}`;
}

export function chipView(status: AiStatus | null): ChipView | null {
  switch (status?.mode) {
    case undefined:
      return null;
    case 'none':
      return { icon: 'key', label: 'Add Meta key', missing: true };
    case 'key':
      return { icon: 'key', label: 'Meta key', missing: false };
    case 'relay': {
      const budget = budgetView(status);
      return budget
        ? { icon: 'ticket', label: 'Access code', left: usd(budget.left), missing: false }
        : { icon: 'ticket', label: 'Access code', missing: false };
    }
  }
}

/** The dialog opens on what is in use; with nothing, on the access code (no Meta account needed). */
export function initialChoice(status: AiStatus | null): PayChoice {
  return status?.mode === 'key' ? 'key' : 'code';
}

/** Checks the key with Meta and saves it. Null when it is saved, else the sentence that says why not. */
export function saveKey(key: string): Promise<string | null> {
  const trimmed = key.trim();
  if (!trimmed) return Promise.resolve('Paste your Meta API key first.');
  return change(() => aiBridge.setKey(trimmed));
}

/** Registers this Mac with the access code. Null when it worked, else the sentence that says why not. */
export function saveCode(code: string): Promise<string | null> {
  const trimmed = code.trim();
  if (!trimmed) return Promise.resolve('Enter your access code first.');
  return change(() => aiBridge.setCode(trimmed));
}

/** Removes the key and the code. Null when they are gone, else why not. */
export function removePayment(): Promise<string | null> {
  return change(() => aiBridge.clear());
}

async function change(send: () => Promise<AiStatus>): Promise<string | null> {
  try {
    setAiStatus(await send());
    return null;
  } catch (error) {
    return error instanceof Error && error.message ? error.message : 'Something went wrong. Try again.';
  }
}

/** Rounded to the cent, so 2 - 0.06 is 1.94 and not 1.9400000000000002. */
function cents(value: number): number {
  return Math.round(value * 100) / 100;
}
