import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';

/**
 * The undo labels chat's changes carry (`chat.<tool>` with values, from `klipolo_chat`'s compiler and the app's picture
 * jobs), so the Undo chip and the history name them: "Undo add the paper plane sticker". `chat.andMore` names a step of
 * several calls by its first ("Make it vertical, and 1 more").
 */
export const CHAT_LABELS: Record<string, MessageDescriptor> = {
  'chat.cutRange': msg({
    id: 'chat.cutRange',
    message: 'Cut {startUs} to {endUs}',
    comment: 'Undo history entry: chat cut a part of the video, from one time to another',
  }),
  'chat.keepOnly': msg({
    id: 'chat.keepOnly',
    message: 'Keep only the parts chosen',
    comment: 'Undo history entry',
  }),
  'chat.removeSilences': msg({
    id: 'chat.removeSilences',
    message: 'Cut the long pauses',
    comment: 'Undo history entry',
  }),
  'chat.removeFillers': msg({
    id: 'chat.removeFillers',
    message: 'Cut the filler words',
    comment: 'Undo history entry',
  }),
  'chat.trimClip': msg({ id: 'chat.trimClip', message: 'Trim a clip', comment: 'Undo history entry' }),
  'chat.splitAt': msg({ id: 'chat.splitAt', message: 'Split at {atUs}', comment: 'Undo history entry: a clip split' }),
  'chat.setFormat': msg({
    id: 'chat.setFormat',
    message: '{format, select, portrait {Make it vertical} square {Make it square} other {Make it landscape}}',
    comment: 'Undo history entry: the video’s main shape changed',
  }),
  'chat.setCaptions': msg({
    id: 'chat.setCaptions',
    message: '{on, select, on {Turn on captions} other {Turn off captions}}',
    comment: 'Undo history entry',
  }),
  'chat.setBackground': msg({
    id: 'chat.setBackground',
    message:
      '{kind, select, blur {Blur the video behind it} color {Use a color background} gradient {Use a gradient background} none {Use a black background} other {Change the background}}',
    comment: 'Undo history entry: what shows behind the video where it doesn’t fill the frame changed',
  }),
  'chat.fixCaptionWords': msg({
    id: 'chat.fixCaptionWords',
    message: 'Change {find} to {replace}',
    comment: 'Undo history entry: a word in the captions corrected',
  }),
  'chat.addMusic': msg({ id: 'chat.addMusic', message: 'Add {file}', comment: 'Undo history entry: music added' }),
  'chat.addSound': msg({ id: 'chat.addSound', message: 'Add {name}', comment: 'Undo history entry: a sound added' }),
  'chat.addImage': msg({ id: 'chat.addImage', message: 'Add {file}', comment: 'Undo history entry: a picture added' }),
  'chat.addText': msg({
    id: 'chat.addText',
    message: 'Add the words {text}',
    comment: 'Undo history entry: words put on screen in a text preset, e.g. "Add the words Day 1 in Lisbon"',
  }),
  'chat.makeTitleCard': msg({
    id: 'chat.makeTitleCard',
    message: 'Add the title {title}',
    comment: 'Undo history entry: a title card with these words added',
  }),
  'chat.makeSticker': msg({
    id: 'chat.makeSticker',
    message: 'Add the {prompt} sticker',
    comment: 'Undo history entry: a drawn sticker placed, e.g. "Add the paper plane sticker"',
  }),
  'chat.titleBackground': msg({
    id: 'chat.titleBackground',
    message: 'Add a title card background',
    comment: 'Undo history entry: a drawn background placed under a title',
  }),
  'chat.andMore': msg({
    id: 'chat.andMore',
    message: '{first}, and {more} more',
    comment: 'Undo history entry for several chat changes in one step: the first, and how many others',
  }),
};
