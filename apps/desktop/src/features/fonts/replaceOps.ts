import type { FontRef } from '../../bindings/FontRef';
import type { Item } from '../../bindings/Item';
import type { MissingFont } from '../../bindings/MissingFont';
import type { Op } from '../../bindings/Op';
import type { ParamValue } from '../../bindings/ParamValue';
import { commitLatest } from '../../state/latest';
import { downloadCjkFont, downloadFont, sameFont } from '../../state/fonts';
import { useProject } from '../../state/project';

/** A row of the Replace popover: one missing font at one face. */
export interface ReplaceRow {
  key: string;
  missing: MissingFont;
  weight: number;
  italic: boolean;
}

/** What the person picked for a row: a font, at the face Rust snapped it to for the row's. */
export interface Choice {
  font: FontRef;
  weight: number;
  italic: boolean;
}

const rowKey = (m: MissingFont, weight: number, italic: boolean) =>
  `${m.cjk ?? ''}|${m.font.family}|${JSON.stringify(m.font.source)}|${weight}|${italic ? 'i' : 'n'}`;

/** One row per missing family (or CJK script) and face. */
export function replaceRows(missing: MissingFont[]): ReplaceRow[] {
  return missing.flatMap((m) =>
    m.weights.map(([weight, italic]) => ({ key: rowKey(m, weight, italic), missing: m, weight, italic })),
  );
}

/** The open project's layers now. */
const layersNow = () => (useProject.getState().snapshot?.project.timeline.tracks ?? []).flatMap((t) => t.items);

/**
 * The open project's text layers that use `m`: a family's by its font, a CJK script's by Rust's list. Those on tracks
 * the export doesn't draw too (hidden, or left out by a solo): replaced everywhere, never left to come back as missing
 * when shown again.
 */
function users(m: MissingFont): Item[] {
  return layersNow().filter(
    (i) =>
      (m.layers.includes(i.id) || m.hiddenLayers.includes(i.id)) &&
      i.content.type === 'text' &&
      (m.cjk !== null || sameFont(i.content.style.font, m.font)),
  );
}

/** A title template's settings with the picks written so far: one op per layer, whatever entries named it. */
interface SceneEdit {
  item: Item;
  params: { [key in string]: ParamValue };
}

/**
 * A title template that takes `m` in a font setting (`sceneParams`: its layer and the setting's key) gets the pick in
 * that setting, never a text style. Built from the project as it is now; the picks for one layer gather in `scenes`
 * (two settings of one layer missing two fonts both take theirs), one op a layer.
 */
function scenePicks(m: MissingFont, pick: (row: string) => Choice | undefined, scenes: Map<string, SceneEdit>) {
  const choice = m.weights.map(([w, i]) => pick(rowKey(m, w, i))).find((c) => c);
  if (!choice || m.sceneParams.length === 0) return;
  for (const item of layersNow()) {
    const keys = m.sceneParams.filter(([layer]) => layer === item.id).map(([, key]) => key);
    if (keys.length === 0 || item.content.type !== 'generator') continue;
    const edit = scenes.get(item.id) ?? { item, params: { ...item.content.params } };
    for (const key of keys) edit.params[key] = choice.font;
    scenes.set(item.id, edit);
  }
}

/**
 * The ops that give each user of a missing font the person's pick for its face, built from the project as it is now
 * (an edit made since stays): its text layers, its title templates' font settings, and the captions' font when they use
 * it (only the family is kept; the style's weight snaps at draw time). A CJK row's pick sets its layers' font whatever
 * they use. One undo step.
 */
export function replaceOps(missing: MissingFont[], pick: (row: string) => Choice | undefined): Op[] {
  const ops: Op[] = [];
  const done = new Set<string>();
  const scenes = new Map<string, SceneEdit>();
  let captions: FontRef | null = null;
  for (const m of missing) {
    for (const item of users(m)) {
      if (done.has(item.id) || item.content.type !== 'text') continue;
      const style = item.content.style;
      const choice = pick(rowKey(m, style.weight, style.italic));
      if (!choice) continue;
      done.add(item.id);
      ops.push({
        type: 'updateItem',
        id: item.id,
        patch: {
          content: { ...item.content, style: { ...style, font: choice.font, weight: choice.weight, italic: choice.italic } },
        },
      });
    }
    if (m.captions && !captions) captions = m.weights.map(([w, i]) => pick(rowKey(m, w, i))).find((c) => c)?.font ?? null;
    scenePicks(m, pick, scenes);
  }
  for (const { item, params } of scenes.values())
    if (item.content.type === 'generator')
      ops.push({ type: 'updateItem', id: item.id, patch: { content: { ...item.content, params } } });
  const settings = useProject.getState().snapshot?.project.captions;
  if (captions && settings) ops.push({ type: 'setCaptions', captions: { ...settings, font: captions } });
  return ops;
}

/** "Replace" in the popover: the picks, as one "Replace fonts" step. Resolves with whether it went in. */
export function commitReplace(missing: MissingFont[], picks: Map<string, Choice>): Promise<boolean> {
  return commitLatest('fontMenu.replace.undo', () => replaceOps(missing, (row) => picks.get(row)));
}

/**
 * "Use {family} {version}": every user of `m` takes the version Google serves now, at its own face (the resolver draws
 * that version's nearest), as one "Replace fonts" step. Never a silent swap: the person's press.
 */
export function commitCurrent(m: MissingFont): Promise<boolean> {
  const current = m.current;
  if (!current) return Promise.resolve(false);
  return commitLatest('fontMenu.replace.undo', () =>
    replaceOps([m], (row) => {
      const face = m.weights.find(([w, i]) => rowKey(m, w, i) === row);
      return face && { font: current, weight: face[0], italic: face[1] };
    }),
  );
}

/** What downloading every missing font in `missing` at its own version takes, or null when one can't be. */
export function downloadAllBytes(missing: MissingFont[]): number | null {
  let all = 0;
  for (const m of missing) {
    if (m.downloadBytes === null) return null;
    all += m.downloadBytes;
  }
  return all;
}

/**
 * Downloads every missing font in turn, each at its project's own version and the faces its users ask for (a CJK Noto
 * per weight). Stops at the first that fails, which it throws.
 */
export async function downloadAll(missing: MissingFont[]): Promise<void> {
  for (const m of missing) {
    const version = m.font.source.type === 'google' ? m.font.source.version : null;
    const asked = new Set<string>();
    for (const [weight, italic] of m.weights) {
      // A CJK Noto's faces are upright: one download a weight.
      const face = m.cjk ? `${weight}` : `${weight}|${italic}`;
      if (asked.has(face)) continue;
      asked.add(face);
      if (m.cjk) await downloadCjkFont(m.cjk, m.font.family, weight);
      else await downloadFont(m.font.family, version, weight, italic);
    }
  }
}
