import { useEffect, useLayoutEffect, useState } from 'react';
import { followAiStatus } from './ai/aiStatus';
import { isCode } from './bridge/call';
import { bridge } from './bridge/commands';
import { cn } from './design/cn';
import { Toaster } from './design/Toaster';
import { TooltipProvider } from './design/Tooltip';
import { useEditorShortcuts } from './features/command-palette/useEditorShortcuts';
import { ExportDialog } from './features/export/ExportDialog';
import { useFinderDrop } from './features/media/useFinderDrop';
import { StartScreen } from './features/start/StartScreen';
import { followSpeech } from './features/timeline/media/speech';
import { useEngineSession } from './features/preview/useEngineSession';
import { Timeline } from './features/timeline/Timeline';
import { Chat } from './layout/Chat';
import { Library } from './layout/Library';
import { TopBar } from './layout/TopBar';
import { errorMessage } from './lib/errors';
import { PlayerBar } from './player/PlayerBar';
import { Preview } from './preview/Preview';
import { SettingsDialog } from './settings/SettingsDialog';
import {
  clearAnalysisOnProjectSwitch,
  followAnalysis,
  forgetRemovedClips,
  offerAgainOnAiModeChange,
} from './state/analysis';
import { followApps } from './state/chat';
import { followEditorView } from './state/editorView';
import { useProject } from './state/project';
import { clearProposalOnProjectSwitch } from './state/proposal';
import { clearRangeOnProjectSwitch } from './state/range';
import { followSaves } from './state/saving';
import { toast } from './state/toasts';
import { keepPlayheadInVideo, keepPlayheadOnReload, restorePlayhead } from './state/transport';

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

/**
 * The editor, while a project is open: the library, the preview, the timeline and chat, and the player bar. Finder
 * drops and the editor's keys work only here.
 */
function Editor() {
  const [filesOver, setFilesOver] = useState(false);
  useFinderDrop(setFilesOver);
  useEditorShortcuts();
  return (
    <>
      <div className="workspace">
        <Library />
        <Preview filesOver={filesOver} />
        <Timeline />
        <Chat />
      </div>
      <PlayerBar />
    </>
  );
}

/**
 * The start screen, until a project opens. The card or button pressed from the keyboard goes with it, so the focus goes
 * on to the project's name in the top bar, where the editor starts. (Its cleanup runs before the screen leaves the page:
 * the focus is still in it.) Coming back from a project (`afterProject`), the focus goes to its title.
 */
function Start({ afterProject }: { afterProject: boolean }) {
  useLayoutEffect(
    () => () => {
      if (!document.querySelector('.start-panel')?.contains(document.activeElement)) return;
      queueMicrotask(() => document.querySelector<HTMLElement>('.topbar .project')?.focus());
    },
    [],
  );
  return <StartScreen takeFocus={afterProject} />;
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
    clearAnalysisOnProjectSwitch();
    offerAgainOnAiModeChange();
    forgetRemovedClips();
    keepPlayheadInVideo();
    keepPlayheadOnReload();
    clearRangeOnProjectSwitch();
    clearProposalOnProjectSwitch();
    // Other apps' calls read the person's view (the selection, the playhead) as Klipolo last heard it.
    followEditorView();
    // What other apps do shows in chat, named for each app.
    void followApps();
    // The Music lane's dips: where someone speaks, asked again as the project changes.
    followSpeech();
    // Development builds only: Control Shift P a stand-in chat, Control Shift W sample words (`dev/devChat`).
    if (import.meta.env.DEV) void import('./dev/devChat').then((dev) => dev.listenForDevKeys());
    // Until a clip's analysis feeds speech: Control Shift S shows sample speech under the music (development only).
    if (import.meta.env.DEV) void import('./dev/sampleSpeech').then((dev) => dev.listenForSampleSpeech());
    void loadOpenProject().finally(() => setAsked(true));
  }, []);
  useEngineSession();
  const open = useProject((s) => s.snapshot !== null);
  // A project has been open in this page: the start screen shows after All projects, not at launch.
  const [hadProject, setHadProject] = useState(false);
  useEffect(() => {
    if (open) setHadProject(true);
  }, [open]);
  return (
    <TooltipProvider>
      <div className={cn('app', !open && 'is-start')}>
        <TopBar />
        {open ? <Editor /> : asked && <Start afterProject={hadProject} />}
        <SettingsDialog />
        <ExportDialog />
        <Toaster />
      </div>
    </TooltipProvider>
  );
}
