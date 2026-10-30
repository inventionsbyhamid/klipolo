import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';

/**
 * The caption looks a text layer can wear, by key and name. Undo labels name a look by its message id
 * (`{ style: { code: 'text.look.box' } }`, `labels.ts`); the look's colours and outline come with the text tools.
 */
export interface TextLookPreset {
  key: string;
  name: MessageDescriptor;
}

export const CAPTION_LOOKS: TextLookPreset[] = [
  {
    key: 'clean',
    name: msg({
      id: 'text.look.clean',
      message: 'Clean',
      comment: 'Caption style: white text with a shadow',
    }),
  },
  {
    key: 'boldOutline',
    name: msg({ id: 'text.look.boldOutline', message: 'Bold outline', comment: 'Caption style' }),
  },
  {
    key: 'highlight',
    name: msg({
      id: 'text.look.highlight',
      message: 'Highlight',
      comment: 'Caption style: text on a yellow band',
    }),
  },
  {
    key: 'box',
    name: msg({ id: 'text.look.box', message: 'Box', comment: 'Caption style: white text in a black box' }),
  },
  {
    key: 'pop',
    name: msg({ id: 'text.look.pop', message: 'Pop', comment: 'Caption style: yellow text with an outline' }),
  },
];
