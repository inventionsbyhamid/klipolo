import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';

/** The undo labels of the Captions section, the canvas block, the captions lane, the word list
 * and the missing fonts' Replace. */
export const CAPTION_LABELS = {
  'captions.on.undo': msg({
    id: 'captions.on.undo',
    message: 'Turn on captions',
    comment: 'Undo history entry',
  }),
  'captions.off.undo': msg({
    id: 'captions.off.undo',
    message: 'Turn off captions',
    comment: 'Undo history entry',
  }),
  'captions.style.undo': msg({
    id: 'captions.style.undo',
    message: 'Change caption style',
    comment: 'Undo history entry',
  }),
  'captions.variant.undo': msg({
    id: 'captions.variant.undo',
    message: 'Change caption look',
    comment: 'Undo history entry; a look is one of a caption style’s variants, e.g. Plate',
  }),
  'captions.look.undo': msg({
    id: 'captions.look.undo',
    message: 'Style caption words',
    comment:
      'Undo history entry: the colour, size, keyword, box, hiding or line breaks of some caption words',
  }),
  'captions.color.undo': msg({
    id: 'captions.color.undo',
    message: 'Change caption colour',
    comment: 'Undo history entry',
  }),
  'captions.font.undo': msg({
    id: 'captions.font.undo',
    message: 'Change caption font',
    comment: 'Undo history entry',
  }),
  'captions.place.undo': msg({
    id: 'captions.place.undo',
    message: 'Move captions',
    comment: 'Undo history entry',
  }),
  'captions.language.undo': msg({
    id: 'captions.language.undo',
    message: 'Change caption language',
    comment: 'Undo history entry',
  }),
  // The missing fonts' Replace, and their "Use {family} {version}".
  'fontMenu.replace.undo': msg({
    id: 'fontMenu.replace.undo',
    message: 'Replace fonts',
    comment: 'Undo history entry: fonts that aren’t on this Mac were replaced by others',
  }),
  'captions.fix.undo': msg({
    id: 'captions.fix.undo',
    message: 'Fix “{from}” to “{to}”',
    comment: 'Undo history entry; {from} is what the captions said, {to} the person’s fix',
  }),
} satisfies Record<string, MessageDescriptor>;
