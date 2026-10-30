import { create } from 'zustand';
import type { CodedError } from '../bindings/CodedError';
import type { EngineInfo } from '../bindings/EngineInfo';
import type { Presenter } from '../bindings/Presenter';

/** off: no editor; starting: attaching; live: frames are drawn; unavailable: the DOM stage stands in. */
export type EngineStatus = 'off' | 'starting' | 'live' | 'unavailable';

interface EngineStore {
  info: EngineInfo | null;
  /** How frames reach the screen: the native layer, or the socket (also after the native one failed). */
  presenter: Presenter;
  status: EngineStatus;
  message: string | null;
  /**
   * The preview waits for a source whose disk hasn't answered for a while (`engine.waitingForDisk`): a quiet note
   * under the stage until a frame is drawn without waiting for it.
   */
  waiting: CodedError | null;
  /** Frames of other sessions belong to a previous project (or opening) and are dropped. */
  session: number;
  /** Bumped by Retry so the session hook attaches again. */
  attempt: number;
  setInfo: (info: EngineInfo) => void;
  setPresenter: (presenter: Presenter) => void;
  setStatus: (status: EngineStatus, message?: string | null) => void;
  setSession: (session: number) => void;
  setWaiting: (waiting: CodedError | null) => void;
  retry: () => void;
  reset: () => void;
}

export const useEngine = create<EngineStore>((set) => ({
  info: null,
  presenter: 'socket',
  status: 'off',
  message: null,
  waiting: null,
  session: 0,
  attempt: 0,
  setInfo: (info) => set({ info, presenter: info.presenter }),
  setPresenter: (presenter) =>
    set((s) => ({ presenter, status: s.status === 'live' ? 'starting' : s.status })),
  setStatus: (status, message = null) => set({ status, message }),
  setSession: (session) => set((s) => ({ session, status: s.status === 'live' ? 'starting' : s.status })),
  setWaiting: (waiting) => set({ waiting }),
  retry: () => set((s) => ({ attempt: s.attempt + 1, status: 'starting', message: null })),
  reset: () =>
    set({ info: null, presenter: 'socket', status: 'off', message: null, waiting: null, session: 0 }),
}));

/** Frames of another session belong to a previous project and are never drawn. */
export function isCurrentFrame(meta: { session: number }): boolean {
  return meta.session === useEngine.getState().session;
}
