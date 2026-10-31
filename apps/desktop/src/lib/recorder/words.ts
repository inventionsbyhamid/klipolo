/**
 * Klipolo's words for the recorder's notes whose English offers what Klipolo doesn't have (a choice of quality, a Home
 * list of takes, Start over), by code. They take the place of the recorder's own wherever the page words a code
 * (`codedMessage`).
 */
export const RECORDER_WORDS: Readonly<Record<string, string>> = {
  'capture.health.dropping': 'Some frames are being dropped. Quitting other busy apps may help.',
  'capture.preflight.diskShort': 'Less than 30 minutes of space left. Free up space to record longer.',
  'capture.startOver.kept':
    "Started over. The last take couldn't be moved to the Trash, so it stays in Movies / Klipolo / Recordings.",
  'capture.startOver.undoFailed': "The last take couldn't be put back. Look for it in the Trash.",
};
