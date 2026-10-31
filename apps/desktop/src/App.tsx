import { useEffect, useState } from 'react';
import { followAiStatus } from './ai/aiStatus';
import { isCode } from './bridge/call';
import { bridge } from './bridge/commands';
import { Toaster } from './design/Toaster';
import { importFiles } from './features/media/useImportMedia';
import { useFinderDrop } from './features/media/useFinderDrop';
import { useEngineSession } from './features/preview/useEngineSession';
import { Chat } from './layout/Chat';
import { Library } from './layout/Library';
import { TimelinePanel } from './layout/TimelinePanel';
import { TopBar } from './layout/TopBar';
import { errorMessage } from './lib/errors';
import { videoDurationUs } from './lib/timeline';
import { PlayerBar } from './player/PlayerBar';
import { Preview } from './preview/Preview';
import { SettingsDialog } from './settings/SettingsDialog';
import { followAnalysis } from './state/analysis';
import { usePlayback } from './state/playback';
import { useProject } from './state/project';
import { followSaves } from './state/saving';
import { toast } from './state/toasts';
import { skip } from './state/transport';

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

/**
 * Space plays and pauses, J and L step 5 s, K pauses, Home and End go to the start and the end (not while typing).
 * Command Z and Shift Command Z undo and redo (a text field keeps its own), Command O adds files.
 */
function useKeys() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const typing = !!target?.closest('input, textarea, [contenteditable="true"]');
      const project = useProject.getState().snapshot?.project;
      if (e.metaKey && !e.ctrlKey && !e.altKey) {
        const key = e.key.toLowerCase();
        if (key === 'o') {
          e.preventDefault();
          void importFiles(null);
        } else if (key === 'z' && !typing) {
          e.preventDefault();
          void (e.shiftKey ? useProject.getState().redo() : useProject.getState().undo());
        }
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey || typing || !project) return;
      const duration = videoDurationUs(project.timeline);
      if (duration <= 0) return;
      const playback = usePlayback.getState();
      if (e.key === ' ' && !target?.closest('button')) {
        e.preventDefault();
        playback.toggle();
      } else if (e.key === 'j' || e.key === 'J') skip(-5_000_000, duration);
      else if (e.key === 'l' || e.key === 'L') skip(5_000_000, duration);
      else if (e.key === 'k' || e.key === 'K') playback.pause();
      else if (e.key === 'Home') playback.seek(0);
      else if (e.key === 'End') playback.seek(duration);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
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
    void openProject();
  }, []);
  useEngineSession();
  useFinderDrop(setFilesOver);
  useKeys();
  return (
    <div className="app">
      <TopBar />
      <div className="workspace">
        <Library />
        <Preview filesOver={filesOver} />
        <TimelinePanel />
        <Chat />
      </div>
      <PlayerBar />
      <SettingsDialog />
      <Toaster />
    </div>
  );
}
