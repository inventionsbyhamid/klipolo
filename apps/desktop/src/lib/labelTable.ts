import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { CAPTION_LABELS } from './captionLabels';

/**
 * Every undo label the UI makes, by message id, one row per id: `labels.ts` words them. A label that
 * shows a value has a placeholder (`Add {effect}`), and the edit passes the value as a param.
 */
export const TABLE = {
  // The Captions section's, the canvas block's and the captions lane's.
  ...CAPTION_LABELS,
  // Edits from the timeline, its keys and menus (`undo.*`: `edit.*` is chat's).
  'undo.split.clip': msg({
    id: 'undo.split.clip',
    message: 'Split clip',
    comment: 'Command and undo history entry',
  }),
  'undo.split.layer': msg({
    id: 'undo.split.layer',
    message: 'Split layer',
    comment: 'Command and undo history entry',
  }),
  'undo.delete.clip': msg({
    id: 'undo.delete.clip',
    message: 'Delete clip',
    comment: 'Command and undo history entry',
  }),
  'undo.delete.clip.note': msg({
    id: 'undo.delete.clip.note',
    message: 'Delete clip. {note}',
    comment: 'Undo history entry; {note} says what it did to transitions, e.g. “A transition is removed.”',
  }),
  'undo.delete.range': msg({
    id: 'undo.delete.range',
    message: 'Delete selection',
    comment: 'Command and undo history entry: the selected time range taken out of the video',
  }),
  'undo.delete.layer': msg({
    id: 'undo.delete.layer',
    message: 'Delete layer',
    comment: 'Command and undo history entry',
  }),
  'undo.cut': msg({ id: 'undo.cut', message: 'Cut', comment: 'Command and undo history entry' }),
  'undo.cut.note': msg({
    id: 'undo.cut.note',
    message: 'Cut. {note}',
    comment: 'Undo history entry; {note} says what it did to transitions, e.g. “A transition is removed.”',
  }),
  'undo.paste': msg({ id: 'undo.paste', message: 'Paste', comment: 'Command and undo history entry' }),
  'undo.duplicate': msg({
    id: 'undo.duplicate',
    message: 'Duplicate',
    comment: 'Command and undo history entry',
  }),
  'undo.pasteEffects': msg({
    id: 'undo.pasteEffects',
    message: 'Paste effects',
    comment: 'Command and undo history entry',
  }),
  'undo.trackUp': msg({
    id: 'undo.trackUp',
    message: 'Move to the track above',
    comment: 'Command and undo history entry',
  }),
  'undo.trackDown': msg({
    id: 'undo.trackDown',
    message: 'Move to the track below',
    comment: 'Command and undo history entry',
  }),
  'undo.attach.keep': msg({
    id: 'undo.attach.keep',
    message: 'Keep with clip',
    comment: 'Command and undo history entry',
  }),
  'undo.attach.free': msg({
    id: 'undo.attach.free',
    message: 'Free from clip',
    comment: 'Command and undo history entry',
  }),
  'undo.separateCamera': msg({
    id: 'undo.separateCamera',
    message: 'Separate camera',
    comment: 'Command and undo history entry',
  }),
  // A recording's sources and markers: `{source}` is a source's name, e.g. Mic, Game sound.
  'undo.source.volume': msg({
    id: 'undo.source.volume',
    message: 'Change {source} volume',
    comment: 'Undo history entry; {source} is a recorded sound, e.g. Mic or Game sound',
  }),
  'undo.source.mute': msg({
    id: 'undo.source.mute',
    message: 'Mute {source}',
    comment: 'Button and undo history entry; {source} is a recorded sound, e.g. Mic or Game sound',
  }),
  'undo.source.unmute': msg({
    id: 'undo.source.unmute',
    message: 'Unmute {source}',
    comment: 'Button and undo history entry; {source} is a recorded sound, e.g. Mic or Game sound',
  }),
  'undo.source.sync': msg({
    id: 'undo.source.sync',
    message: 'Sync {source}',
    comment: 'Undo history entry: the recorded sound was moved earlier or later to match the picture',
  }),
  'undo.source.separate': msg({
    id: 'undo.source.separate',
    message: 'Separate {source}',
    comment:
      'Menu item and undo history entry: the recorded sound becomes its own layer; {source} is e.g. Mic',
  }),
  'undo.source.centre': msg({
    id: 'undo.source.centre',
    message: 'Centre {source}',
    comment: 'Undo history entry: a sound recorded on one side now plays in both ears; {source} is e.g. Mic',
  }),
  'undo.source.fillGaps': msg({
    id: 'undo.source.fillGaps',
    message: "Fill gaps with the camera's sound",
    comment:
      'Undo history entry: the camera’s own sound plays where the separately recorded sound has nothing',
  }),
  'undo.source.noFillGaps': msg({
    id: 'undo.source.noFillGaps',
    message: 'Leave gaps silent',
    comment: 'Undo history entry: where the separately recorded sound has nothing, nothing plays',
  }),
  'undo.source.asRecorded': msg({
    id: 'undo.source.asRecorded',
    message: 'Play {source} as recorded',
    comment: 'Undo history entry: a centred sound plays as it was recorded again; {source} is e.g. Mic',
  }),
  'undo.record': msg({
    id: 'undo.record',
    message: 'Add recording',
    comment: 'Undo history entry: a finished recording added to the project',
  }),
  'undo.autoZoom': msg({
    id: 'undo.autoZoom',
    message: 'Add auto zoom',
    comment: "Undo history entry: zooms that follow a new recording's clicks, added right after it",
  }),
  'undo.marker.add': msg({ id: 'undo.marker.add', message: 'Add marker', comment: 'Undo history entry' }),
  'undo.marker.remove': msg({
    id: 'undo.marker.remove',
    message: 'Remove marker',
    comment: 'Undo history entry: a flag marking a moment of a recording',
  }),
  // Media and text.
  'media.import': msg({
    id: 'media.import',
    message: 'Import media',
    comment: 'Command and undo history entry',
  }),
  'media.add': msg({ id: 'media.add', message: 'Add media', comment: 'Command and undo history entry' }),
  'media.opacity.undo': msg({
    id: 'media.opacity.undo',
    message: 'Change opacity',
    comment: 'Undo history entry',
  }),
  'media.corners.undo': msg({
    id: 'media.corners.undo',
    message: 'Round corners',
    comment: 'Undo history entry',
  }),
  // The library's media commands (Rust builds them) and the inspector's fields.
  'media.music.add': msg({ id: 'media.music.add', message: 'Add music', comment: 'Undo history entry' }),
  'media.sound.add': msg({ id: 'media.sound.add', message: 'Add sound', comment: 'Undo history entry' }),
  'media.image.add': msg({ id: 'media.image.add', message: 'Add image', comment: 'Undo history entry' }),
  'media.text.add': msg({ id: 'media.text.add', message: 'Add text', comment: 'Undo history entry' }),
  'media.title.add': msg({ id: 'media.title.add', message: 'Add title', comment: 'Undo history entry' }),
  'music.duck.undo': msg({
    id: 'music.duck.undo',
    message: 'Change ducking',
    comment: 'Undo history entry: how much quieter the music gets while people talk',
  }),
  'text.background.undo': msg({
    id: 'text.background.undo',
    message: 'Change text background',
    comment: 'Undo history entry',
  }),
  'layout.position.undo': msg({ id: 'layout.position.undo', message: 'Move to a position', comment: 'Undo history entry' }),
  'timing.start.undo': msg({ id: 'timing.start.undo', message: 'Change start', comment: 'Undo history entry' }),
  'timing.length.undo': msg({ id: 'timing.length.undo', message: 'Change length', comment: 'Undo history entry' }),
  'title.edit.undo': msg({ id: 'title.edit.undo', message: 'Edit title', comment: 'Undo history entry' }),
  'title.accent.undo': msg({ id: 'title.accent.undo', message: 'Change accent colour', comment: 'Undo history entry' }),
  'text.add': msg({ id: 'text.add', message: 'Add text', comment: 'Command and undo history entry' }),
  'text.edit.undo': msg({ id: 'text.edit.undo', message: 'Edit text', comment: 'Undo history entry' }),
  'text.remove.undo': msg({
    id: 'text.remove.undo',
    message: 'Remove text',
    comment: 'Undo history entry: a title whose words were all deleted on the video',
  }),
  'text.size.undo': msg({ id: 'text.size.undo', message: 'Change text size', comment: 'Undo history entry' }),
  'text.font.undo': msg({ id: 'text.font.undo', message: 'Change font', comment: 'Undo history entry' }),
  'text.bold.undo': msg({ id: 'text.bold.undo', message: 'Make text bold', comment: 'Undo history entry' }),
  'text.regular.undo': msg({
    id: 'text.regular.undo',
    message: 'Make text regular',
    comment: 'Undo history entry',
  }),
  'text.align.undo': msg({ id: 'text.align.undo', message: 'Align text', comment: 'Undo history entry' }),
  'text.animation.undo': msg({
    id: 'text.animation.undo',
    message: 'Change text animation',
    comment: 'Undo history entry',
  }),
  'text.color.undo': msg({
    id: 'text.color.undo',
    message: 'Change text colour',
    comment: 'Undo history entry',
  }),
  'text.highlight.add.undo': msg({
    id: 'text.highlight.add.undo',
    message: 'Add highlight',
    comment: 'Undo history entry',
  }),
  'text.highlight.remove.undo': msg({
    id: 'text.highlight.remove.undo',
    message: 'Remove highlight',
    comment: 'Undo history entry',
  }),
  'text.highlight.color.undo': msg({
    id: 'text.highlight.color.undo',
    message: 'Change highlight colour',
    comment: 'Undo history entry',
  }),
  'text.outline.add.undo': msg({
    id: 'text.outline.add.undo',
    message: 'Add outline',
    comment: 'Undo history entry',
  }),
  'text.outline.remove.undo': msg({
    id: 'text.outline.remove.undo',
    message: 'Remove outline',
    comment: 'Undo history entry',
  }),
  'text.outline.color.undo': msg({
    id: 'text.outline.color.undo',
    message: 'Change outline colour',
    comment: 'Undo history entry',
  }),
  'text.outline.width.undo': msg({
    id: 'text.outline.width.undo',
    message: 'Change outline width',
    comment: 'Undo history entry',
  }),
  'text.shadow.add.undo': msg({
    id: 'text.shadow.add.undo',
    message: 'Add shadow',
    comment: 'Undo history entry',
  }),
  'text.shadow.remove.undo': msg({
    id: 'text.shadow.remove.undo',
    message: 'Remove shadow',
    comment: 'Undo history entry',
  }),
  'text.look.apply.undo': msg({
    id: 'text.look.apply.undo',
    message: 'Apply {style} style',
    comment: 'Undo history entry; {style} is a caption style, e.g. Bold outline',
  }),
  // The project: its name, format versions and looks.
  'editor.rename.undo': msg({
    id: 'editor.rename.undo',
    message: 'Rename project',
    comment: 'Undo history entry',
  }),
  'format.add.undo': msg({
    id: 'format.add.undo',
    message: 'Add {ratio} version',
    comment: 'Undo history entry; {ratio} is an aspect ratio such as 9:16',
  }),
  'format.remove.undo': msg({
    id: 'format.remove.undo',
    message: 'Remove {ratio} version',
    comment: 'Undo history entry; {ratio} is an aspect ratio such as 9:16',
  }),
  'format.switch.undo': msg({
    id: 'format.switch.undo',
    message: 'Switch to {ratio}',
    comment: 'Undo history entry: the main format changed; {ratio} is an aspect ratio such as 9:16',
  }),
  'looks.apply.undo': msg({
    id: 'looks.apply.undo',
    message: 'Apply {look} look',
    comment: 'Undo history entry; {look} is the look’s name, e.g. Warm, Mono',
  }),
  'looks.clear.undo': msg({
    id: 'looks.clear.undo',
    message: 'Remove look',
    comment: 'Undo history entry: the clips no longer have a colour look',
  }),
  // A clip's or layer's tools.
  'clip.fades.undo': msg({ id: 'clip.fades.undo', message: 'Change fades', comment: 'Undo history entry' }),
  'clip.framing.fit.undo': msg({
    id: 'clip.framing.fit.undo',
    message: 'Fit video',
    comment: 'Undo history entry',
  }),
  'clip.framing.fill.undo': msg({
    id: 'clip.framing.fill.undo',
    message: 'Fill video',
    comment: 'Undo history entry',
  }),
  'clip.framing.focus.undo': msg({
    id: 'clip.framing.focus.undo',
    message: 'Change fill focus',
    comment: 'Undo history entry: which part of a filled video stays in view',
  }),
  'crop.undo': msg({ id: 'crop.undo', message: 'Crop video', comment: 'Undo history entry: edges cut off a video' }),
  'crop.reset.undo': msg({
    id: 'crop.reset.undo',
    message: 'Remove crop',
    comment: 'Undo history entry: a video shown whole again',
  }),
  // The background behind the video where it doesn't fill the frame.
  'background.kind.undo': msg({
    id: 'background.kind.undo',
    message: 'Change background',
    comment: 'Undo history entry: what shows behind the video changed',
  }),
  'background.color.undo': msg({
    id: 'background.color.undo',
    message: 'Change background color',
    comment: 'Undo history entry',
  }),
  'background.picture.undo': msg({
    id: 'background.picture.undo',
    message: 'Change background picture',
    comment: 'Undo history entry',
  }),
  'background.padding.undo': msg({
    id: 'background.padding.undo',
    message: 'Change padding',
    comment: 'Undo history entry: the space round the video changed',
  }),
  // The cursor layer: its look, and following it in a vertical or square version.
  'undo.cursorStyle': msg({
    id: 'undo.cursorStyle',
    message: 'Change cursor',
    comment: 'Undo history entry',
  }),
  'undo.cursorStyle.all': msg({
    id: 'undo.cursorStyle.all',
    message: 'Use this cursor in every clip',
    comment: 'Undo history entry',
  }),
  'undo.followCursor': msg({
    id: 'undo.followCursor',
    message: 'Follow the cursor',
    comment: 'Undo history entry: the vertical or square crop follows the mouse pointer',
  }),
  'undo.followCursor.off': msg({
    id: 'undo.followCursor.off',
    message: 'Stop following the cursor',
    comment: 'Undo history entry',
  }),
  'undo.followCursor.closer': msg({
    id: 'undo.followCursor.closer',
    message: 'Change how close it follows',
    comment: 'Undo history entry: how far the following crop zooms in',
  }),
  // What an edit does to transitions, said after it: aloud, and at the end of a speed change's entry (a chat
  // change's names it as a value, `chat.setClip.note`).
  'timeline.refit': msg({
    id: 'timeline.refit',
    message:
      '{removed, plural, =0 {{shortened, plural, =0 {} =1 {A transition is shortened.} other {# transitions are shortened.}}} =1 {A transition is removed{shortened, plural, =0 {.} =1 { and one is shortened.} other { and # are shortened.}}} other {# transitions are removed{shortened, plural, =0 {.} =1 { and one is shortened.} other { and # are shortened.}}}}',
    comment: 'What an edit does to transitions, said after what it did, e.g. “2 transitions are removed and one is shortened.”',
  }),
  'speed.undo': msg({ id: 'speed.undo', message: 'Change speed', comment: 'Undo history entry' }),
  'speed.undo.note': msg({
    id: 'speed.undo.note',
    message: 'Change speed. {note}',
    comment: 'Undo history entry; {note} says what it did to transitions, e.g. “A transition is shortened.”',
  }),
  'volume.undo': msg({ id: 'volume.undo', message: 'Change volume', comment: 'Undo history entry' }),
  'volume.mute.undo': msg({
    id: 'volume.mute.undo',
    message: '{noun, select, clip {Mute clip} video {Mute video layer} sound {Mute sound} other {Mute layer}}',
    comment: 'Undo history entry',
  }),
  'volume.unmute.undo': msg({
    id: 'volume.unmute.undo',
    message:
      '{noun, select, clip {Unmute clip} video {Unmute video layer} sound {Unmute sound} other {Unmute layer}}',
    comment: 'Undo history entry',
  }),
  'sound.effect.undo': msg({
    id: 'sound.effect.undo',
    message: 'Change sound effect',
    comment: 'Undo history entry',
  }),
  'zoom.undo': msg({ id: 'zoom.undo', message: 'Change zoom', comment: 'Undo history entry' }),
  'camera.layout.undo': msg({
    id: 'camera.layout.undo',
    message: 'Change camera layout',
    comment: 'Undo history entry',
  }),
  'camera.shape.undo': msg({
    id: 'camera.shape.undo',
    message: 'Change camera shape',
    comment: 'Undo history entry',
  }),
  'camera.border.show.undo': msg({
    id: 'camera.border.show.undo',
    message: 'Show camera border',
    comment: 'Undo history entry',
  }),
  'camera.border.hide.undo': msg({
    id: 'camera.border.hide.undo',
    message: 'Hide camera border',
    comment: 'Undo history entry',
  }),
  'camera.mirror.undo': msg({
    id: 'camera.mirror.undo',
    message: 'Mirror camera',
    comment: 'Undo history entry',
  }),
  'camera.unmirror.undo': msg({
    id: 'camera.unmirror.undo',
    message: 'Unmirror camera',
    comment: 'Undo history entry',
  }),
  'media.mirror.undo': msg({
    id: 'media.mirror.undo',
    message: 'Mirror video layer',
    comment: 'Undo history entry',
  }),
  'media.unmirror.undo': msg({
    id: 'media.unmirror.undo',
    message: 'Unmirror video layer',
    comment: 'Undo history entry',
  }),
  // Effects: `{effect}` is the contribution id, worded as its name (`contribNames`).
  'effect.add.undo': msg({
    id: 'effect.add.undo',
    message: 'Add {effect}',
    comment: 'Undo history entry; {effect} is an effect’s name, e.g. Blur',
  }),
  'effect.change.undo': msg({
    id: 'effect.change.undo',
    message: 'Change {effect}',
    comment: 'Undo history entry: a setting of the effect changed; {effect} is its name, e.g. Blur',
  }),
  'effect.on.undo': msg({
    id: 'effect.on.undo',
    message: 'Turn {effect} on',
    comment: 'Undo history entry; {effect} is an effect’s name, e.g. Blur',
  }),
  'effect.off.undo': msg({
    id: 'effect.off.undo',
    message: 'Turn {effect} off',
    comment: 'Undo history entry; {effect} is an effect’s name, e.g. Blur',
  }),
  'effect.up.undo': msg({
    id: 'effect.up.undo',
    message: 'Move {effect} up',
    comment: 'Undo history entry; {effect} is an effect’s name, e.g. Blur',
  }),
  'effect.down.undo': msg({
    id: 'effect.down.undo',
    message: 'Move {effect} down',
    comment: 'Undo history entry; {effect} is an effect’s name, e.g. Blur',
  }),
  'effect.remove.undo': msg({
    id: 'effect.remove.undo',
    message: 'Remove {effect}',
    comment: 'Button and undo history entry; {effect} is an effect’s name, e.g. Blur',
  }),
  // The canvas: `{layer}` is text, image, video or camera.
  'gizmo.move.undo': msg({
    id: 'gizmo.move.undo',
    message:
      '{layer, select, text {Move text} image {Move image} video {Move video layer} camera {Move camera} other {Move layer}}',
    comment: 'Undo history entry',
  }),
  'gizmo.resize.undo': msg({
    id: 'gizmo.resize.undo',
    message:
      '{layer, select, text {Resize text} image {Resize image} video {Resize video layer} camera {Resize camera} other {Resize layer}}',
    comment: 'Undo history entry',
  }),
  'gizmo.rotate.undo': msg({
    id: 'gizmo.rotate.undo',
    message:
      '{layer, select, text {Rotate text} image {Rotate image} video {Rotate video layer} camera {Rotate camera} other {Rotate layer}}',
    comment: 'Undo history entry',
  }),
  'gizmo.nudge.undo': msg({
    id: 'gizmo.nudge.undo',
    message:
      '{layer, select, text {Nudge text} image {Nudge image} video {Nudge video layer} camera {Nudge camera} other {Nudge layer}}',
    comment: 'Undo history entry: the layer was moved with the arrow keys',
  }),
  // The timeline: drags, tracks and transitions.
  'timeline.move.clip.undo': msg({
    id: 'timeline.move.clip.undo',
    message: 'Move clip',
    comment: 'Undo history entry',
  }),
  'timeline.move.layer.undo': msg({
    id: 'timeline.move.layer.undo',
    message: 'Move layer',
    comment: 'Undo history entry',
  }),
  'timeline.move.layerNewTrack.undo': msg({
    id: 'timeline.move.layerNewTrack.undo',
    message: 'Move layer to a new track',
    comment: 'Undo history entry',
  }),
  'timeline.trim.clip.undo': msg({
    id: 'timeline.trim.clip.undo',
    message: 'Trim clip',
    comment: 'Undo history entry',
  }),
  'timeline.trim.layer.undo': msg({
    id: 'timeline.trim.layer.undo',
    message: 'Trim layer',
    comment: 'Undo history entry',
  }),
  // Footage moved across a cut or through a clip, the video's length kept.
  'timeline.roll.undo': msg({
    id: 'timeline.roll.undo',
    message: 'Roll',
    comment: 'Undo history entry: the cut between two clips moved, one clip longer and the other shorter',
  }),
  'timeline.slip.undo': msg({
    id: 'timeline.slip.undo',
    message: 'Slip',
    comment: "Undo history entry: a clip shows other footage of its file, keeping its place and length",
  }),
  'timeline.slide.undo': msg({
    id: 'timeline.slide.undo',
    message: 'Slide',
    comment: 'Undo history entry: a clip moved between its neighbours, which got longer and shorter to make room',
  }),
  // A selected group's edits: `{count}` is how many clips and layers.
  'timeline.group.delete': msg({
    id: 'timeline.group.delete',
    message: '{count, plural, one {Delete # item} other {Delete # items}}',
    comment: 'Command and undo history entry: several selected clips and layers deleted at once',
  }),
  'timeline.group.move': msg({
    id: 'timeline.group.move',
    message: '{count, plural, one {Move # item} other {Move # items}}',
    comment: 'Undo history entry: several selected layers moved together',
  }),
  'timeline.group.paste': msg({
    id: 'timeline.group.paste',
    message: '{count, plural, one {Paste # item} other {Paste # items}}',
    comment: 'Undo history entry',
  }),
  'timeline.group.duplicate': msg({
    id: 'timeline.group.duplicate',
    message: '{count, plural, one {Duplicate # item} other {Duplicate # items}}',
    comment: 'Undo history entry',
  }),
  'timeline.group.cut': msg({
    id: 'timeline.group.cut',
    message: '{count, plural, one {Cut # item} other {Cut # items}}',
    comment: 'Undo history entry: several selected clips and layers cut to the clipboard',
  }),
  'timeline.group.mute': msg({
    id: 'timeline.group.mute',
    message: '{count, plural, one {Mute # item} other {Mute # items}}',
    comment: 'Undo history entry',
  }),
  'timeline.group.unmute': msg({
    id: 'timeline.group.unmute',
    message: '{count, plural, one {Unmute # item} other {Unmute # items}}',
    comment: 'Undo history entry',
  }),
  'timeline.group.volume': msg({
    id: 'timeline.group.volume',
    message: '{count, plural, one {Change the volume of # item} other {Change the volume of # items}}',
    comment: 'Undo history entry',
  }),
  'timeline.group.pasteEffects': msg({
    id: 'timeline.group.pasteEffects',
    message: '{count, plural, one {Paste effects onto # item} other {Paste effects onto # items}}',
    comment: 'Undo history entry',
  }),
  'timeline.track.add': msg({
    id: 'timeline.track.add',
    message: 'Add track',
    comment: 'Undo history entry',
  }),
  'timeline.track.rename': msg({
    id: 'timeline.track.rename',
    message: 'Rename track',
    comment: 'Command and undo history entry',
  }),
  'timeline.track.move': msg({
    id: 'timeline.track.move',
    message: 'Move track',
    comment: 'Undo history entry',
  }),
  'timeline.track.delete': msg({
    id: 'timeline.track.delete',
    message: 'Delete track',
    comment: 'Menu item, command and undo history entry',
  }),
  'timeline.track.hide': msg({
    id: 'timeline.track.hide',
    message: 'Hide track',
    comment: 'Button and undo history entry',
  }),
  'timeline.track.show': msg({
    id: 'timeline.track.show',
    message: 'Show track',
    comment: 'Button and undo history entry',
  }),
  'timeline.track.mute': msg({
    id: 'timeline.track.mute',
    message: 'Mute track',
    comment: 'Button and undo history entry',
  }),
  'timeline.track.unmute': msg({
    id: 'timeline.track.unmute',
    message: 'Unmute track',
    comment: 'Button and undo history entry',
  }),
  'timeline.track.lock': msg({
    id: 'timeline.track.lock',
    message: 'Lock track',
    comment: 'Button and undo history entry',
  }),
  'timeline.track.unlock': msg({
    id: 'timeline.track.unlock',
    message: 'Unlock track',
    comment: 'Button and undo history entry',
  }),
  'timeline.track.solo': msg({
    id: 'timeline.track.solo',
    message: 'Solo track',
    comment: 'Button and undo history entry: only soloed tracks play and show',
  }),
  'timeline.track.unsolo': msg({
    id: 'timeline.track.unsolo',
    message: 'Unsolo track',
    comment: 'Button and undo history entry: every track plays again',
  }),
  // A lane's header and options act on every track the lane draws: `{lane}` is the lane's name, a nested code.
  'timeline.lane.overlays': msg({
    id: 'timeline.lane.overlays',
    message: 'Overlays',
    comment: 'The lane of titles, pictures and video layers',
  }),
  'timeline.lane.sounds': msg({ id: 'timeline.lane.sounds', message: 'Sound effects' }),
  'timeline.lane.music': msg({ id: 'timeline.lane.music', message: 'Music' }),
  'timeline.lane.hide': msg({
    id: 'timeline.lane.hide',
    message: 'Hide {lane}',
    comment: "Button, menu item and undo history entry; {lane} is a timeline lane's name, e.g. Overlays",
  }),
  'timeline.lane.show': msg({
    id: 'timeline.lane.show',
    message: 'Show {lane}',
    comment: "Button, menu item and undo history entry; {lane} is a timeline lane's name, e.g. Overlays",
  }),
  'timeline.lane.mute': msg({
    id: 'timeline.lane.mute',
    message: 'Mute {lane}',
    comment: "Button, menu item and undo history entry; {lane} is a timeline lane's name, e.g. Music",
  }),
  'timeline.lane.unmute': msg({
    id: 'timeline.lane.unmute',
    message: 'Unmute {lane}',
    comment: "Button, menu item and undo history entry; {lane} is a timeline lane's name, e.g. Music",
  }),
  'timeline.lane.solo': msg({
    id: 'timeline.lane.solo',
    message: 'Solo {lane}',
    comment: "Menu item and undo history entry: only soloed lanes play and show; {lane} is a timeline lane's name",
  }),
  'timeline.lane.unsolo': msg({
    id: 'timeline.lane.unsolo',
    message: 'Unsolo {lane}',
    comment: "Menu item and undo history entry: every lane plays again; {lane} is a timeline lane's name",
  }),
  'timeline.lane.lock': msg({
    id: 'timeline.lane.lock',
    message: 'Lock {lane}',
    comment: "Menu item and undo history entry; {lane} is a timeline lane's name, e.g. Music",
  }),
  'timeline.lane.unlock': msg({
    id: 'timeline.lane.unlock',
    message: 'Unlock {lane}',
    comment: "Menu item and undo history entry; {lane} is a timeline lane's name, e.g. Music",
  }),
  'timeline.track.soloOff': msg({
    id: 'timeline.track.soloOff',
    message: 'Turn off solo',
    comment: 'Command and undo history entry: every soloed track plays normally again',
  }),
  'transition.add': msg({
    id: 'transition.add',
    message: 'Add transition',
    comment: 'Command and undo history entry',
  }),
  'transition.remove': msg({
    id: 'transition.remove',
    message: 'Remove transition',
    comment: 'Button, command and undo history entry',
  }),
  'transition.change.undo': msg({
    id: 'transition.change.undo',
    message: 'Change transition',
    comment: 'Undo history entry',
  }),
  'transition.length.undo': msg({
    id: 'transition.length.undo',
    message: 'Change transition length',
    comment: 'Undo history entry',
  }),
} satisfies Record<string, MessageDescriptor>;

/**
 * Labels for other apps' steps (MCP). Klipolo's host makes them, so they use its namespace (`mcp`) and stay out of
 * `TABLE`: a step folded from one app's earlier steps, and how a label whose step another app made (its params carry
 * the app's name as `client`) names the app.
 */
export const MCP_LABELS: Record<string, MessageDescriptor> = {
  'mcp.fromEarlier': msg({
    id: 'mcp.fromEarlier',
    message: 'Earlier changes from {client}',
    comment: 'Undo history entry: several older changes another app made, kept as one step; {client} is the app',
  }),
};

/** A label whose step another app made: "Make it vertical (from Example Client)". */
export const FROM_CLIENT = msg({
  id: 'history.fromApp',
  message: '{label} (from {client})',
  comment: 'Undo history entry another app made: {label} is the change, {client} the app',
});
