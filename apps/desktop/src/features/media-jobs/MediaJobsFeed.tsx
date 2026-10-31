import { useEffect } from 'react';
import { mediaJobs } from '../../bridge/mediaJobs';
import { useMediaJobs } from '../../state/mediaJobs';
import { useUi } from '../../state/ui';

/**
 * Editing copies while the editor is open. The person's setting goes first: Rust's is app-wide and starts on, and the
 * editor opens before the engine attaches, so copies turned off never start one in between (it is sent again on
 * attach). Then the list is followed, listening before it is listed so no change between the two is missed. Its events
 * draw only the top bar's pill (and its popover while it is open).
 */
export function MediaJobsFeed() {
  useEffect(() => {
    mediaJobs.setAuto(useUi.getState().autoCopies).catch(() => undefined);
    const { listen, load } = useMediaJobs.getState();
    const stop = listen();
    void load();
    return stop;
  }, []);
  return null;
}
