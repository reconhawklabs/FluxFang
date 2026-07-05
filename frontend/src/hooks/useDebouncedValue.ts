// Tiny generic debounce hook — used by `RuleBuilder` (Task 9.2) to delay the
// `/api/emitters/preview` fetch until the user has paused editing the rule,
// rather than firing a request on every keystroke.
import { useEffect, useState } from 'react';

export function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);

  return debounced;
}
