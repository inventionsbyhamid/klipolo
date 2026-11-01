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
  'chat.setClip': msg({
    id: 'chat.setClip',
    message:
      '{change, select, speed {Change speed} volume {Change volume} mute {{noun, select, sound {Mute sound} video {Mute video layer} other {Mute clip}}} unmute {{noun, select, sound {Unmute sound} video {Unmute video layer} other {Unmute clip}}} fit {Fit video} fill {Fill video} other {{noun, select, sound {Change sound} video {Change video layer} other {Change clip}}}}',
    comment:
      'Undo history entry: chat changed a clip, a song (noun sound) or a video layer (noun video) as its card does, in the card’s own words',
  }),
  'chat.setClip.note': msg({
    id: 'chat.setClip.note',
    message: '{change, select, speed {Change speed} other {Change clip}}. {note}',
    comment: 'Undo history entry; {note} says what it did to transitions, e.g. “A transition is shortened.”',
  }),
  'chat.setTransition': msg({
    id: 'chat.setTransition',
    message:
      '{change, select, add {{count, plural, =1 {Add transition} other {Add # transitions}}} remove {{count, plural, =1 {Remove transition} other {Remove # transitions}}} length {{count, plural, =1 {Change transition length} other {Change # transition lengths}}} other {{count, plural, =1 {Change transition} other {Change # transitions}}}}',
    comment: 'Undo history entry: chat added, changed or removed the transitions at cuts',
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
  'chat.setCameraMode': msg({
    id: 'chat.setCameraMode',
    message:
      '{mode, select, hidden {Hide the camera} splitBottom {Put the camera under the screen} other {Show the camera as a bubble}}',
    comment: 'Undo history entry: how a recording’s camera shows changed',
  }),
  'chat.adjustCamera': msg({
    id: 'chat.adjustCamera',
    message: 'Adjust the camera',
    comment: 'Undo history entry: the camera bubble moved, resized or restyled',
  }),
  'chat.detachCamera': msg({
    id: 'chat.detachCamera',
    message: 'Separate the camera',
    comment: 'Undo history entry: a recording’s camera made a layer of its own',
  }),
  'chat.muteSource': msg({
    id: 'chat.muteSource',
    message:
      '{mute, select, on {Mute} other {Unmute}} the {source, select, computerSound {computer sound} gameSound {game sound} videoSound {camera sound} other {mic}}',
    comment: 'Undo history entry: a recorded sound muted or unmuted, e.g. "Mute the mic"',
  }),
  'chat.setSourceVolume': msg({
    id: 'chat.setSourceVolume',
    message:
      'Set the {source, select, computerSound {computer sound} gameSound {game sound} videoSound {camera sound} other {mic}} to {percent}%',
    comment: 'Undo history entry: a recorded sound’s volume, in percent of how it was recorded, e.g. "Set the mic to 50%"',
  }),
  'chat.separateSound': msg({
    id: 'chat.separateSound',
    message:
      'Separate the {source, select, computerSound {computer sound} gameSound {game sound} videoSound {camera sound} other {mic}}',
    comment: 'Undo history entry: a recorded sound put on a track of its own, e.g. "Separate the mic"',
  }),
  'chat.andMore': msg({
    id: 'chat.andMore',
    message: '{first}, and {more} more',
    comment: 'Undo history entry for several chat changes in one step: the first, and how many others',
  }),
};
