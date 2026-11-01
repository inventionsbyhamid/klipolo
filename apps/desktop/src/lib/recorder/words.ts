/**
 * Klipolo's words for the recorder's notes whose English offers what Klipolo doesn't have (a choice of quality, a Home
 * list of takes, Start over), says "we", or leaves out what to do next, by code. They take the place of the recorder's
 * own wherever the page words a code (`codedMessage`).
 */
export const RECORDER_WORDS: Readonly<Record<string, string>> = {
  'capture.preflight.bluetoothMic':
    "Your headphones' microphone lowers the sound quality while recording, so Klipolo picked {fallback}.",
  'capture.preflight.cameraWithScreen':
    "This computer can't record your camera at the same time as the screen. Klipolo will record the screen without your camera.",
  'capture.preflight.micMissing': "The microphone you chose isn't connected, so Klipolo picked {fallback}.",
  'capture.preflight.profilePreset':
    "The quality you chose for {app} isn't available on this computer now, so Klipolo picked the best one for it.",
  'capture.health.dropping': 'Some frames are being dropped. Quitting other busy apps may help.',
  'capture.preflight.diskShort': 'Less than 30 minutes of space left. Free up space to record longer.',
  'capture.startOver.kept':
    "Started over. The last take couldn't be moved to the Trash, so it stays in Movies / Klipolo / Recordings.",
  'capture.startOver.undoFailed': "The last take couldn't be put back. Look for it in the Trash.",
};
