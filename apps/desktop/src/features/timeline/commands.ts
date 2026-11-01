/**
 * What other features run on the timeline (the shortcuts, the player bar): its commands and its zoom. Import from
 * here, not from the timeline's internals.
 */
export { duplicate } from './keys/clipboardActions';
export { EditFrames } from './EditFrames';
export { deleteGroup, duplicateGroup, copyGroup, itemsWord, muteGroup, pasteEffectsGroup, setGroupVolume } from './keys/groupActions';
export { allEffects, allSound, countOf, groupMuted, groupVolume, groupVolumeOps, membersOf } from './keys/groupOps';
export { trackStepOps } from './keys/keyOps';
export { noteMutedTrack, stepTrack } from './keys/layerActions';
export { deleteBlock, deleteRange, markRange } from './keys/timelineActions';
export { speedNote } from './model/refit';
export { removeTransition, transitionName, transitionNow, transitionOp } from './keys/transitionActions';
export { useTimelineCommands } from './useTimelineCommands';
export { useZoomTarget, zoomRange, zoomTo, zoomViewUs } from './zoomControl';
