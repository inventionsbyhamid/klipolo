import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { EditorView } from '../bindings/EditorView';
import type { ExternalReply } from '../bindings/ExternalReply';
import type { McpStatus } from '../bindings/McpStatus';
import { call } from './call';

export type { UnlistenFn };

/**
 * Other apps (MCP clients, such as an AI agent) editing the open project through Klipolo: the Settings switch, the
 * config an app needs to start Klipolo's bridge, what each app did (for chat), and the person's view, which an app's
 * calls read as "this" and "here".
 */

/** Whether Klipolo listens for other apps now, the apps used recently, and the config to copy. */
export function mcpStatus(): Promise<McpStatus> {
  return call<McpStatus>('mcp_status');
}

/**
 * Turns other apps' access on or off, and saves it. Rejects with one plain sentence when Klipolo couldn't start
 * listening: it stays off.
 */
export function setMcpEnabled(on: boolean): Promise<McpStatus> {
  return call<McpStatus>('mcp_set_enabled', { on });
}

/** Each change, proposal, set of moments or picture another app made (`mcp-reply`). */
export function onMcpReply(handler: (reply: ExternalReply) => void): Promise<UnlistenFn> {
  return listen<ExternalReply>('mcp-reply', (event) => handler(event.payload));
}

/** The apps used recently, by name, whenever that changes (`mcp-apps`). */
export function onMcpApps(handler: (apps: string[]) => void): Promise<UnlistenFn> {
  return listen<string[]>('mcp-apps', (event) => handler(event.payload));
}

/** How long the view has to stay put before it is sent: a drag or a scrub sends only where it ends. */
export const VIEW_SETTLE_MS = 250;

let waiting: ReturnType<typeof setTimeout> | null = null;

/**
 * Tells Klipolo the person's view (the range selected, the playhead, the overlay selected, the format shown) once it
 * has stayed put for 250 ms: a newer view replaces one still waiting. Nothing is said when it can't be sent.
 */
export function sendEditorView(view: EditorView) {
  if (waiting) clearTimeout(waiting);
  waiting = setTimeout(() => {
    waiting = null;
    void call<void>('editor_view', { view }).catch(() => undefined);
  }, VIEW_SETTLE_MS);
}
