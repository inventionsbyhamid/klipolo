import { useEffect, useState } from 'react';

/** The screen's device pixel ratio, followed as the window moves between screens. */
export function useDevicePixelRatio(): number {
  const [dpr, setDpr] = useState(() => (typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1));
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const query = window.matchMedia(`(resolution: ${dpr}dppx)`);
    const update = () => setDpr(window.devicePixelRatio || 1);
    query.addEventListener?.('change', update);
    return () => query.removeEventListener?.('change', update);
  }, [dpr]);
  return dpr;
}
