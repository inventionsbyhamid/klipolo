import { useEffect } from 'react';
import { Chat } from './layout/Chat';
import { Library } from './layout/Library';
import { TimelinePanel } from './layout/TimelinePanel';
import { TopBar } from './layout/TopBar';
import { PlayerBar } from './player/PlayerBar';
import { getSession, seek, skip, startSession, togglePlay } from './player/session';
import { Preview } from './preview/Preview';

let started = false;

/** Space plays and pauses, J and L step 5 s, Home goes to the start (not while typing). */
function useTransportKeys() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (e.metaKey || e.ctrlKey || e.altKey || target?.closest('input, textarea, [contenteditable="true"]')) return;
      if (!getSession().info || !getSession().project) return;
      if (e.key === ' ' && !target?.closest('button')) {
        e.preventDefault();
        void togglePlay();
      } else if (e.key === 'j' || e.key === 'J') {
        void skip(-5_000_000);
      } else if (e.key === 'l' || e.key === 'L') {
        void skip(5_000_000);
      } else if (e.key === 'Home') {
        void seek(0);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
}

/** The window. Only the preview and the transport work so far. */
export function App() {
  useEffect(() => {
    // Once per page load (React's strict mode runs effects twice in development).
    if (started) return;
    started = true;
    void startSession();
  }, []);
  useTransportKeys();
  return (
    <div className="app">
      <TopBar />
      <div className="workspace">
        <Library />
        <Preview />
        <TimelinePanel />
        <Chat />
      </div>
      <PlayerBar />
    </div>
  );
}
