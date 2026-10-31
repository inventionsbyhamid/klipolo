/**
 * What other features run on the timeline (the shortcuts, the player bar): its commands and its zoom. Import from
 * here, not from the timeline's internals.
 */
export { duplicate } from './keys/clipboardActions';
export { trackStepOps } from './keys/keyOps';
export { stepTrack } from './keys/layerActions';
export { deleteBlock, deleteRange, markRange } from './keys/timelineActions';
export { speedNote } from './model/refit';
export { removeTransition, transitionName, transitionNow, transitionOp } from './keys/transitionActions';
export { useTimelineCommands } from './useTimelineCommands';
export { useZoomTarget, zoomRange, zoomTo, zoomViewUs } from './zoomControl';
