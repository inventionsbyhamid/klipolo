import { t } from '@lingui/core/macro';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { FontRef } from '../../bindings/FontRef';
import { isCode } from '../../bridge/call';
import { errorMessage } from '../../lib/errors';
import { consentToGoogleFonts, loadGoogleFonts, useFonts } from '../../state/fonts';
import { toast } from '../../state/toasts';

/** A Google download the person asked for: what it takes, how it runs, and what follows once the font is here. */
export interface GoogleWanted {
  /** The family, for what is said when the component that asked is gone. */
  family: string;
  bytes: number;
  run: () => Promise<FontRef>;
  apply?: (font: FontRef) => void;
  /** The control that asked: the keyboard goes back to it once the consent closes. */
  from?: HTMLElement | null;
}

/**
 * A Google download the person asked for: a menu pick of a family not downloaded. Before the person's one consent it
 * asks in place (`asking`, shown as `GoogleConsent`); with it, it runs at once, and once the font is here its `apply`
 * runs (a pick: one undo step, "Change font"). A failure is said in place (`error`) while what asked is `shown` (the
 * font menu open); a cancel quietly. When what asked is gone or closed by then, a failure is a toast, and a font that
 * arrived after its component went says it's ready to pick. Once the consent closes, the keyboard goes back to the
 * control that asked (`from`).
 */
export function useGooglePick(shown: () => boolean = () => true) {
  const [asking, setAsking] = useState<GoogleWanted | null>(null);
  const [error, setError] = useState<string | null>(null);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);
  // Once the consent has gone from the page, the keyboard goes back to what asked (never left on the page).
  const focusBack = useRef<HTMLElement | null>(null);
  useEffect(() => {
    if (asking || !focusBack.current) return;
    if (focusBack.current.isConnected) focusBack.current.focus();
    focusBack.current = null;
  }, [asking]);
  const start = async (w: GoogleWanted) => {
    setError(null);
    try {
      const font = await w.run();
      if (live.current) w.apply?.(font);
      else if (w.apply) {
        const family = w.family;
        toast.info(
          t({
            id: 'fontMenu.readyLater',
            message: `${family} is downloaded. Pick it again to use it.`,
            comment: '{family} is a Google font that finished downloading after its menu was closed',
          }),
        );
      }
    } catch (e) {
      if (isCode(e, 'fonts.download.cancelled')) return;
      if (live.current && shown()) setError(errorMessage(e));
      else toast.error(errorMessage(e));
    }
  };
  const clearError = useCallback(() => setError(null), []);
  return {
    /** The download waiting for the consent. */
    asking,
    error,
    clearError,
    /** The person asked for `w`: at once once they have consented, else after the consent (asked of Rust first). */
    pick: async (w: GoogleWanted) => {
      if (!useFonts.getState().googleLoaded) await loadGoogleFonts();
      if (useFonts.getState().googleConsented) void start(w);
      else if (live.current) setAsking(w);
    },
    /** "Download ({size})" in the consent: confirmed once, then the download. */
    confirm: async () => {
      const waiting = asking;
      if (!waiting) return;
      focusBack.current = waiting.from ?? null;
      setAsking(null);
      try {
        await consentToGoogleFonts();
      } catch (e) {
        setError(errorMessage(e));
        return;
      }
      await start(waiting);
    },
    cancelAsk: () => {
      focusBack.current = asking?.from ?? null;
      setAsking(null);
    },
  };
}
