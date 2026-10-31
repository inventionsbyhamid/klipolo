import { useEffect, useState } from 'react';
import { followAiStatus } from './ai/aiStatus';
import { isCode } from './bridge/call';
import { bridge } from './bridge/commands';
import { Toaster } from './design/Toaster';
import { TooltipProvider } from './design/Tooltip';
import { useEditorShortcuts } from './features/command-palette/useEditorShortcuts';
import { ExportDialog } from './features/export/ExportDialog';
import { useFinderDrop } from './features/media/useFinderDrop';
import { useEngineSession } from './features/preview/useEngineSession';
import { Timeline } from './features/timeline/Timeline';
import { Chat } from './layout/Chat';
import { Library } from './layout/Library';
import { TopBar } from './layout/TopBar';
import { errorMessage } from './lib/errors';
import { PlayerBar } from './player/PlayerBar';
import { Preview } from './preview/Preview';
import { SettingsDialog } from './settings/SettingsDialog';
import { clearAnalysisOnProjectSwitch, followAnalysis, forgetRemovedClips } from './state/analysis';
import { useProject } from './state/project';
import { clearProposalOnProjectSwitch } from './state/proposal';
import { clearRangeOnProjectSwitch } from './state/range';
import { followSaves } from './state/saving';
import { toast } from './state/toasts';
import { keepPlayheadInVideo } from './state/transport';

/** The name and formats a first launch's project gets: all three formats, 16:9 first. */
const NEW_PROJECT = { name: 'Untitled project', formats: ['landscape', 'portrait', 'square'] as const };

let started = false;

/** The project Rust opened at launch (the one open when the app last quit), else a new one on a first launch. */
async function openProject() {
  try {
    await useProject.getState().load(await bridge.snapshot());
  } catch (error) {
    if (!isCode(error, 'project.none')) return toast.error(errorMessage(error));
    await useProject.getState().create(NEW_PROJECT.name, [...NEW_PROJECT.formats]);
  }
}

/** The window. */
export function App() {
  const [filesOver, setFilesOver] = useState(false);
  useEffect(() => {
    // Once per page load (React's strict mode runs effects twice in development).
    if (started) return;
    started = true;
    void followSaves();
    void followAiStatus();
    void followAnalysis();
    clearAnalysisOnProjectSwitch();
    forgetRemovedClips();
    keepPlayheadInVideo();
    clearRangeOnProjectSwitch();
    clearProposalOnProjectSwitch();
    // Until chat proposes cuts: Control Shift P shows a sample proposal (development builds only).
    if (import.meta.env.DEV) void import('./dev/sampleProposal').then((dev) => dev.listenForSampleProposal());
    void openProject();
  }, []);
  useEngineSession();
  useFinderDrop(setFilesOver);
  useEditorShortcuts();
  const open = useProject((s) => s.snapshot !== null);
  return (
    <TooltipProvider>
      <div className="app">
        <TopBar />
        <div className="workspace">
          <Library />
          <Preview filesOver={filesOver} />
          {open ? <Timeline /> : <section className="panel timeline" aria-label="Timeline" />}
          <Chat />
        </div>
        <PlayerBar />
        <SettingsDialog />
        <ExportDialog />
        <Toaster />
      </div>
    </TooltipProvider>
  );
}
