import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { useDebouncedValue } from './useDebouncedValue';

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

test('holds the initial value immediately, then updates only after the delay once changes stop', () => {
  const { result, rerender } = renderHook(({ value }) => useDebouncedValue(value, 400), {
    initialProps: { value: 'a' },
  });

  expect(result.current).toBe('a');

  rerender({ value: 'b' });
  // Not yet updated — still within the debounce window.
  act(() => {
    vi.advanceTimersByTime(200);
  });
  expect(result.current).toBe('a');

  // A further change resets the timer (this is what "debounced" means: only
  // the final value survives, not every intermediate keystroke).
  rerender({ value: 'c' });
  act(() => {
    vi.advanceTimersByTime(200);
  });
  expect(result.current).toBe('a');

  act(() => {
    vi.advanceTimersByTime(200);
  });
  expect(result.current).toBe('c');
});
