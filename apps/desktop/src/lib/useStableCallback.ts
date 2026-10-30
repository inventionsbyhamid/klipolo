import { useCallback, useLayoutEffect, useRef } from 'react';

/** A function whose identity never changes but always calls the latest `fn` (props of `memo` blocks). */
export function useStableCallback<A extends unknown[], R>(fn: (...args: A) => R): (...args: A) => R {
  const ref = useRef(fn);
  useLayoutEffect(() => {
    ref.current = fn;
  });
  return useCallback((...args: A) => ref.current(...args), []);
}
