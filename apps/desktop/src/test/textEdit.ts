import { screen } from '@testing-library/react';
import { mockIPC } from '@tauri-apps/api/mocks';
import { useProject } from '../state/project';
import { fixturePatch, fixtureSnapshot } from './fixtures';

/** An edit as Rust receives it. */
export type Tx = { label: string; ops: { type: string; [k: string]: unknown }[] };

/** What Rust heard while a title was typed on the canvas, for the canvas editor's tests. */
export const heard = {
  applied: [] as Tx[],
  /** `engine_set_editing`'s items, in order. */
  editing: [] as (string | null)[],
  /** The order of the commits, the preview's hiding and showing, and the quit flush's answer. */
  calls: [] as string[],
  reset() {
    this.applied = [];
    this.editing = [];
    this.calls = [];
  },
};

/**
 * Rust's side of everything the stage asks for while a title is typed, with `apply` answering edits (by default each
 * lands on the copy as it is).
 */
export function rustForText(
  apply: (tx: Tx) => unknown = () => fixturePatch(useProject.getState().snapshot!),
) {
  mockIPC((cmd, args) => {
    if (cmd === 'project_apply') {
      heard.calls.push('apply');
      heard.applied.push((args as { tx: Tx }).tx);
      return apply((args as { tx: Tx }).tx);
    }
    if (cmd === 'project_snapshot') return fixtureSnapshot();
    if (cmd === 'engine_set_editing') {
      const item = (args as { item: string | null }).item;
      heard.editing.push(item);
      heard.calls.push(item === null ? 'show' : `hide ${item}`);
    }
    if (cmd === 'app_quit_flushed') heard.calls.push(`flushed ${(args as { token: number }).token}`);
    if (cmd === 'fonts_resolve')
      return (args as { keys: unknown[] }).keys.map(() => ({
        id: 'inter#0',
        alias: 'kl:b:inter',
        weight: 700,
        italic: false,
        standIn: false,
        cjk: { han: false, kana: false, hangul: false },
        stretch: 5,
        tooLarge: false,
      }));
    if (cmd === 'fonts_face') return new ArrayBuffer(8);
    return null;
  });
}

/** The canvas editor's textarea. */
export const editor = () => screen.getByRole('textbox', { name: 'Text, editing' }) as HTMLTextAreaElement;

/** The fixture title's box on the canvas (selected or not). */
export const titleBox = () => screen.getByRole('button', { name: /^(Select |Text: )Set up billing/ });
