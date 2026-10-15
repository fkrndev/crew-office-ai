import { useEffect, useState } from 'react';

/** Wall clock that re-renders its caller every `everyMs` while `active` (and not
 *  at all otherwise). */
export function useNow(active: boolean, everyMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const iv = setInterval(() => setNow(Date.now()), everyMs);
    return () => clearInterval(iv);
  }, [active, everyMs]);
  return now;
}
