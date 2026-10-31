import { createContext } from 'react';
import type { Viewport } from '../useViewport';
import type { ZoomRest } from './zoomRest';

/**
 * The lanes' render window (`useViewport`), for what draws only its part inside it (`BlockMedia`). Outside a timeline
 * the window is empty, so nothing draws.
 */
export const ViewportContext = createContext<Viewport>({ fromUs: 0, toUs: 0, widthPx: 0, scrollPx: 0 });

/** The timeline's zoom gestures (`createZoomRest`): block media waits for a zoom to rest. None: it never waits. */
export const ZoomRestContext = createContext<ZoomRest | null>(null);
