import type { EffectInstance } from '../../bindings/EffectInstance';
import type { Project } from '../../bindings/Project';
import { useProject } from '../../state/project';

/** How many of the video's own colours the colour popover offers ("In this video"). */
export const VIDEO_MAX = 8;

/** A colour as the project stores one: `#RRGGBB`, or `#RRGGBBAA` with an alpha. */
const COLOR = /^#[0-9a-f]{6}([0-9a-f]{2})?$/i;

/** A colour as "In this video" offers it: its red, green and blue, uppercase `#RRGGBB`. */
const rgbOnly = (hex: string) => hex.slice(0, 7).toUpperCase();

/**
 * The colours the video uses: its text layers' fill, highlight and outline, its title templates' colours, the
 * captions' accent, the caption words' own colours, every clip and layer effect setting that is a colour, and the
 * background's colour (and its gradient's end) while the background shows them. Each once, as uppercase
 * `#RRGGBB` (the alpha is left out: a pick keeps the setting's own), most used first (a tie in the order met), none
 * with the red, green and blue of one in `exclude` (the presets and the style's colours, offered already), at most
 * `VIDEO_MAX`. One pass over the project, then one over its distinct colours: O(layers + effects + looks) in time and
 * memory.
 */
export function projectColors(project: Project, exclude: readonly string[]): string[] {
  /** Each colour's uses, in the order first met. */
  const uses = new Map<string, number>();
  const count = (value: unknown) => {
    if (typeof value !== 'string' || !COLOR.test(value)) return;
    const hex = rgbOnly(value);
    uses.set(hex, (uses.get(hex) ?? 0) + 1);
  };
  const settings = (effects: readonly EffectInstance[]) => {
    for (const effect of effects) for (const value of Object.values(effect.params)) count(value);
  };
  for (const clip of project.timeline.main) settings(clip.effects);
  for (const track of project.timeline.tracks) {
    for (const item of track.items) {
      if (item.content.type === 'text') {
        const { color, background, outline } = item.content.style;
        count(color);
        count(background);
        count(outline?.color);
      } else if (item.content.type === 'generator') {
        for (const value of Object.values(item.content.params)) count(value);
      }
      settings(item.effects);
    }
  }
  count(project.captions.accent);
  for (const word of project.wordLooks ?? []) count(word.look.color);
  // Every project holds a background colour; only a solid or gradient background shows it (none: nobody chose it).
  const { background } = project;
  if (background.kind === 'solid' || background.kind === 'gradient') count(background.color);
  if (background.kind === 'gradient') count(background.colorEnd);

  const skip = new Set(exclude.map(rgbOnly));
  /** The most used so far, most first: each goes in after every one used as often, so a tie keeps the order met. */
  const top: { hex: string; n: number }[] = [];
  for (const [hex, n] of uses) {
    if (skip.has(hex)) continue;
    let at = top.length;
    while (at > 0 && top[at - 1].n < n) at--;
    if (at === VIDEO_MAX) continue;
    top.splice(at, 0, { hex, n });
    if (top.length > VIDEO_MAX) top.pop();
  }
  return top.map((c) => c.hex);
}

/**
 * `projectColors` of the open project as it is now (none without one), read once, with no subscription: the colour
 * popover takes it as it opens and keeps it while open, so it never rescans while a drag moves or a
 * change lands, and nothing moves under the pointer or focus.
 */
export function projectColorsNow(exclude: readonly string[]): string[] {
  const project = useProject.getState().snapshot?.project;
  return project ? projectColors(project, exclude) : [];
}
