import { i18n, type I18n, type MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import type { BuiltinCaptionStyle } from '../bindings/BuiltinCaptionStyle';

/**
 * The built-in caption styles' names, keyed by their word: Rust's one list of styles (`BuiltinCaptionStyle`), so a new
 * style file asks for its name here. Neutral names only.
 */
const CAPTION_STYLES = {
  bold: msg({
    id: 'contrib.caption.bold',
    message: 'Bold',
    comment: 'A caption style: big words that pop in',
  }),
  loud: msg({
    id: 'contrib.caption.loud',
    message: 'Loud',
    comment: 'A caption style: one or two comic words with a thick outline',
  }),
  boxed: msg({
    id: 'contrib.caption.boxed',
    message: 'Boxed',
    comment: 'A caption style: sentence case on rounded boxes',
  }),
  karaoke: msg({
    id: 'contrib.caption.karaoke',
    message: 'Karaoke',
    comment: 'A caption style: the spoken word lights up',
  }),
  highlight: msg({
    id: 'contrib.caption.highlight',
    message: 'Highlight',
    comment: 'A caption style: a coloured box follows each word',
  }),
  clean: msg({
    id: 'contrib.caption.clean',
    message: 'Clean',
    comment: 'A caption style: calm subtitles',
  }),
  arcade: msg({
    id: 'contrib.caption.arcade',
    message: 'Arcade',
    comment: 'A caption style: condensed glowing capitals, for gameplay',
  }),
} satisfies Record<BuiltinCaptionStyle, MessageDescriptor>;

/**
 * The built-in contributions' names, keyed by id: an undo label's params carry the id
 * (`{ effect: 'klipolo.builtin/effect.blur' }`), never the English name, and the name is worded in the language shown.
 * One row per id in `builtin-catalog.json` (a test holds them equal). Packs keep their manifest names.
 */
const NAMES: Record<string, MessageDescriptor> = {
  'klipolo.builtin/audio.telephone': msg({
    id: 'contrib.audio.telephone',
    message: 'Telephone',
    comment: 'A sound effect: a voice as if on the phone',
  }),
  'klipolo.builtin/audio.voice': msg({
    id: 'contrib.audio.voice',
    message: 'Voice',
    comment: 'A sound effect: a clearer voice',
  }),
  'klipolo.builtin/audio.warm': msg({ id: 'contrib.audio.warm', message: 'Warm', comment: 'A sound effect' }),
  'klipolo.builtin/effect.blur': msg({ id: 'contrib.effect.blur', message: 'Blur', comment: 'A video effect' }),
  'klipolo.builtin/effect.drop-shadow': msg({
    id: 'contrib.effect.drop-shadow',
    message: 'Drop shadow',
    comment: 'A video effect: a soft shadow behind the shape of a layer',
  }),
  'klipolo.builtin/effect.glow': msg({
    id: 'contrib.effect.glow',
    message: 'Glow',
    comment: 'A video effect: a soft coloured light around the shape of a layer',
  }),
  'klipolo.builtin/effect.mask': msg({
    id: 'contrib.effect.mask',
    message: 'Shape mask',
    comment: 'A video effect: shows a layer in a circle, a rounded square…',
  }),
  'klipolo.builtin/effect.pixelate': msg({
    id: 'contrib.effect.pixelate',
    message: 'Pixelate',
    comment: 'A video effect',
  }),
  'klipolo.builtin/effect.sparkle': msg({
    id: 'contrib.effect.sparkle',
    message: 'Sparkle',
    comment: 'A video effect: twinkling glints on text or a logo',
  }),
  'klipolo.builtin/effect.vignette': msg({
    id: 'contrib.effect.vignette',
    message: 'Vignette',
    comment: 'A video effect: darker corners',
  }),
  'klipolo.builtin/generator.aurora': msg({
    id: 'contrib.generator.aurora',
    message: 'Aurora',
    comment: 'A layer that draws northern lights drifting over a night sky',
  }),
  'klipolo.builtin/generator.dust': msg({
    id: 'contrib.generator.dust',
    message: 'Dust and bokeh',
    comment: 'A layer that draws soft specks of light floating upward, some out of focus',
  }),
  'klipolo.builtin/generator.gradient': msg({
    id: 'contrib.generator.gradient',
    message: 'Gradient',
    comment: 'A layer that draws a colour gradient',
  }),
  'klipolo.builtin/generator.grain': msg({
    id: 'contrib.generator.grain',
    message: 'Film grain',
    comment: 'A layer of moving film grain, laid over the video',
  }),
  'klipolo.builtin/generator.light-leak': msg({
    id: 'contrib.generator.light-leak',
    message: 'Light leak',
    comment: 'A layer of warm light drifting in from the edges, as in an old film camera',
  }),
  'klipolo.builtin/generator.waves': msg({
    id: 'contrib.generator.waves',
    message: 'Waves',
    comment: 'A layer that draws an ocean seen from a little above, with sun glitter',
  }),
  'klipolo.builtin/look.cinema': msg({
    id: 'contrib.look.cinema',
    message: 'Cinema',
    comment: 'A colour look',
  }),
  'klipolo.builtin/look.cool': msg({ id: 'contrib.look.cool', message: 'Cool', comment: 'A colour look' }),
  'klipolo.builtin/look.mono': msg({
    id: 'contrib.look.mono',
    message: 'Mono',
    comment: 'A colour look: black and white',
  }),
  'klipolo.builtin/look.romcom': msg({
    id: 'contrib.look.romcom',
    message: 'Rom-com',
    comment: 'A colour look, after romantic comedies',
  }),
  'klipolo.builtin/look.vintage': msg({
    id: 'contrib.look.vintage',
    message: 'Vintage',
    comment: 'A colour look',
  }),
  'klipolo.builtin/look.vivid': msg({ id: 'contrib.look.vivid', message: 'Vivid', comment: 'A colour look' }),
  'klipolo.builtin/look.warm': msg({ id: 'contrib.look.warm', message: 'Warm', comment: 'A colour look' }),
  'klipolo.builtin/motion.blur-in': msg({
    id: 'contrib.motion.blur-in',
    message: 'Blur',
    comment: 'An animation: the layer comes into focus as it appears',
  }),
  'klipolo.builtin/motion.bounce': msg({
    id: 'contrib.motion.bounce',
    message: 'Bounce',
    comment: 'An animation: the layer bounces in',
  }),
  'klipolo.builtin/motion.drop': msg({
    id: 'contrib.motion.drop',
    message: 'Drop',
    comment: 'An animation: the layer drops in from above',
  }),
  'klipolo.builtin/motion.fade': msg({
    id: 'contrib.motion.fade',
    message: 'Fade',
    comment: 'An animation: the layer fades in or out',
  }),
  'klipolo.builtin/motion.float': msg({
    id: 'contrib.motion.float',
    message: 'Float',
    comment: 'A looping animation: the layer drifts gently up and down',
  }),
  'klipolo.builtin/motion.pop': msg({
    id: 'contrib.motion.pop',
    message: 'Pop',
    comment: 'An animation: the layer pops in or out',
  }),
  'klipolo.builtin/motion.pulse': msg({
    id: 'contrib.motion.pulse',
    message: 'Pulse',
    comment: 'A looping animation: the layer gently grows and shrinks',
  }),
  'klipolo.builtin/motion.shake': msg({
    id: 'contrib.motion.shake',
    message: 'Shake',
    comment: 'A looping animation: the layer shakes',
  }),
  'klipolo.builtin/motion.slide': msg({
    id: 'contrib.motion.slide',
    message: 'Slide',
    comment: 'An animation: the layer slides in or out',
  }),
  'klipolo.builtin/motion.spin': msg({
    id: 'contrib.motion.spin',
    message: 'Spin',
    comment: 'A looping animation: the layer turns round',
  }),
  'klipolo.builtin/motion.typewriter': msg({
    id: 'contrib.motion.typewriter',
    message: 'Typewriter',
    comment: 'An animation: letters appear one by one',
  }),
  'klipolo.builtin/motion.wiggle': msg({
    id: 'contrib.motion.wiggle',
    message: 'Wiggle',
    comment: 'A looping animation: the layer tilts back and forth',
  }),
  'klipolo.builtin/motion.zoom': msg({
    id: 'contrib.motion.zoom',
    message: 'Zoom',
    comment: 'An animation: the layer grows in or shrinks out',
  }),
  'klipolo.builtin/transition.dip': msg({
    id: 'contrib.transition.dip',
    message: 'Dip to colour',
    comment: 'A transition through a plain colour',
  }),
  'klipolo.builtin/transition.dissolve': msg({
    id: 'contrib.transition.dissolve',
    message: 'Dissolve',
    comment: 'A transition',
  }),
  'klipolo.builtin/transition.push': msg({
    id: 'contrib.transition.push',
    message: 'Push',
    comment: 'A transition',
  }),
  'klipolo.builtin/transition.wipe': msg({
    id: 'contrib.transition.wipe',
    message: 'Wipe',
    comment: 'A transition',
  }),
  ...Object.fromEntries(
    Object.entries(CAPTION_STYLES).map(([word, name]) => [`klipolo.builtin/caption.${word}`, name]),
  ),
};

/** The ids this table names (a test holds them equal to the catalogue's). */
export const NAMED_CONTRIBS: readonly string[] = Object.keys(NAMES);

/** A built-in contribution's name in the language shown (or `inI18n`'s); null for anything else. */
export function contribName(id: string, inI18n: I18n = i18n): string | null {
  // Own keys only: a clip named "constructor" is a name, not a contribution.
  return Object.hasOwn(NAMES, id) ? inI18n._(NAMES[id]) : null;
}
