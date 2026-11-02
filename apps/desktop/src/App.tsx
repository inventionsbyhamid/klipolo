import { useEffect, useState } from 'react';
import type { Snapshot } from './bindings/Snapshot';
import { followAiStatus } from './ai/aiStatus';
import { isCode } from './bridge/call';
import { bridge } from './bridge/commands';
import { cn } from './design/cn';
import { Toaster } from './design/Toaster';
import { TooltipProvider } from './design/Tooltip';
import { useEditorShortcuts } from './features/command-palette/useEditorShortcuts';
import { useRecordShortcut } from './features/command-palette/useRecordShortcut';
import { ExportDialog } from './features/export/ExportDialog';
import { useFinderDrop } from './features/media/useFinderDrop';
import { MediaJobsFeed } from './features/media-jobs/MediaJobsFeed';
import { RecordDialog } from './features/record/RecordDialog';
import { StartScreen } from './features/start/StartScreen';
import { followSpeech } from './features/timeline/media/speech';
import { startFontsFeed } from './state/fonts';
import { useEngineSession } from './features/preview/useEngineSession';
import { Timeline } from './features/timeline/Timeline';
import { Chat } from './layout/Chat';
import { Library } from './layout/Library';
import { TopBar } from './layout/TopBar';
import { errorMessage } from './lib/errors';
import { PlayerBar } from './player/PlayerBar';
import { Preview } from './preview/Preview';
import { SettingsDialog } from './settings/SettingsDialog';
import { useThemeSync } from './settings/useThemeSync';
import {
  clearAnalysisOnProjectSwitch,
  followAnalysis,
  forgetRemovedClips,
  offerAgainOnAiModeChange,
} from './state/analysis';
import { followApps } from './state/chat';
import { followEditorView } from './state/editorView';
import { followFullScreen, useFullScreen } from './state/fullScreen';
import { keepLooping } from './state/loop';
import { followPosters } from './state/posters';
import { useProject } from './state/project';
import { clearProposalOnProjectSwitch } from './state/proposal';
import { clearRangeOnProjectSwitch } from './state/range';
import { followRecorder, recordYourScreen, recoverLeftTakes, reopenWhereLeft } from './state/recorder';
import { followSaves } from './state/saving';
import { toast } from './state/toasts';
import { keepPlayheadInVideo, keepPlayheadOnReload, restorePlayhead } from './state/transport';
import { followVolume } from './state/volume';

let started = false;

/**
 * The project Rust has open as the page loads: none at launch, so the start screen shows; after a reload of the page
 * (development, a WebKit restart), the one being edited, with the playhead where it was. Settles once Rust has answered.
 */
async function loadOpenProject() {
  try {
    await useProject.getState().load(await bridge.snapshot());
    restorePlayhead();
  } catch (error) {
    if (!isCode(error, 'project.none')) toast.error(errorMessage(error));
  }
}

/** Which opening of which project is open: a new one each time a project opens, the same through its edits. */
const openingOf = (snapshot: Snapshot | null) =>
  snapshot ? `${snapshot.project.id}:${snapshot.opening ?? ''}` : null;

/**
 * Each project that opens (from a card, Open recent or New project) puts the keyboard on the editor, its preview, so
 * Space plays at once rather than pressing the button that had the focus (the project's name, whose menu opened it).
 * Focus left anywhere else (a dialog, a field) stays there. The top bar's buttons are a Shift Tab away.
 */
function useKeysOnEditor() {
  const opening = useProject((s) => openingOf(s.snapshot));
  useEffect(() => {
    if (opening === null) return;
    // After the menu that opened it has given the focus back to its trigger (it does so a moment after it closes).
    const timer = setTimeout(() => {
      const active = document.activeElement;
      const free = !active || active === document.body || active.closest('.topbar') !== null;
      if (free) document.querySelector<HTMLElement>('.workspace .stage')?.focus({ preventScroll: true });
    });
    return () => clearTimeout(timer);
  }, [opening]);
}

/**
 * The editor, while a project is open: the library, the preview, the timeline and chat, and the player bar. Finder
 * drops and the editor's keys work only here, and the project's editing copies are followed (the top bar's pill).
 */
function Editor() {
  const [filesOver, setFilesOver] = useState(false);
  useFinderDrop(setFilesOver);
  useEditorShortcuts();
  useKeysOnEditor();
  return (
    <>
      <div className="workspace">
        <Library />
        <Preview filesOver={filesOver} />
        <Timeline />
        <Chat />
      </div>
      <PlayerBar />
      <MediaJobsFeed />
    </>
  );
}

/**
 * The start screen, until a project opens. The card or button pressed from the keyboard goes with it, and the focus
 * goes on to the editor (`useKeysOnEditor`). Coming back from a project (`afterProject`), the focus goes to its title.
 */
function Start({ afterProject }: { afterProject: boolean }) {
  return <StartScreen onRecord={recordYourScreen} takeFocus={afterProject} />;
}

/**
 * The window: the start screen until a project opens, then the editor's layout.
 * The top bar, the dialogs and the toasts stay throughout.
 */
export function App() {
  // Until Rust says what is open, neither screen shows: a reload with a project open never flashes the start screen.
  const [asked, setAsked] = useState(false);
  useEffect(() => {
    // Once per page load (React's strict mode runs effects twice in development).
    if (started) return;
    started = true;
    void followSaves();
    void followAiStatus();
    void followAnalysis();
    // The Record dialog's previews and takes, and every take that lands.
    void followRecorder();
    clearAnalysisOnProjectSwitch();
    offerAgainOnAiModeChange();
    forgetRemovedClips();
    keepPlayheadInVideo();
    keepPlayheadOnReload();
    // Loop: a forward run goes back to the selected range's start at its end.
    keepLooping();
    // The player bar's volume reaches the preview as it changes.
    followVolume();
    // Full screen preview ends with the project, or when the window leaves full screen by itself.
    followFullScreen();
    clearRangeOnProjectSwitch();
    clearProposalOnProjectSwitch();
    // Other apps' calls read the person's view (the selection, the playhead) as Klipolo last heard it.
    followEditorView();
    // What other apps do shows in chat, named for each app.
    void followApps();
    // Downloaded and installed fonts: a download, a removal or this Mac's fonts arriving redraws the texts in them.
    startFontsFeed();
    // The Music lane's dips: where someone speaks, asked again as the project changes.
    followSpeech();
    // The start screen's posters, made in the background: each card swaps to its own as it lands.
    void followPosters();
    // Development builds only: Control Shift P a stand-in chat, Control Shift W sample words (`dev/devChat`).
    if (import.meta.env.DEV) void import('./dev/devChat').then((dev) => dev.listenForDevKeys());
    // Until a clip's analysis feeds speech: Control Shift S shows sample speech under the music (development only).
    if (import.meta.env.DEV) void import('./dev/sampleSpeech').then((dev) => dev.listenForSampleSpeech());
    // Back from Reopen Klipolo, the project the person had open opens again, with the Record dialog. Then takes the
    // last run left behind (Klipolo quit or crashed during them) land, each in a new project.
    void loadOpenProject()
      .then(reopenWhereLeft)
      .finally(() => {
        setAsked(true);
        void recoverLeftTakes();
      });
  }, []);
  // Settings' Appearance on the page and the window, from before the first paint.
  useThemeSync();
  useEngineSession();
  useRecordShortcut();
  const open = useProject((s) => s.snapshot !== null);
  const full = useFullScreen((s) => s.on);
  // A project has been open in this page: the start screen shows after All projects, not at launch.
  const [hadProject, setHadProject] = useState(false);
  useEffect(() => {
    if (open) setHadProject(true);
  }, [open]);
  return (
    <TooltipProvider>
      <div className={cn('app', !open && 'is-start', open && full && 'is-full')}>
        <TopBar />
        {open ? <Editor /> : asked && <Start afterProject={hadProject} />}
        <SettingsDialog />
        <ExportDialog />
        <RecordDialog />
        <Toaster />
      </div>
    </TooltipProvider>
  );
}
