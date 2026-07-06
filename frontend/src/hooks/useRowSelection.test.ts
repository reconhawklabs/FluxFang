import { act, renderHook } from '@testing-library/react';
import { expect, test } from 'vitest';
import { useRowSelection } from './useRowSelection';

test('toggle adds/removes a single id', () => {
  const { result } = renderHook(() => useRowSelection(['a', 'b']));

  act(() => result.current.toggle('a'));
  expect(result.current.selected.has('a')).toBe(true);

  act(() => result.current.toggle('a'));
  expect(result.current.selected.has('a')).toBe(false);
});

test('allSelected is true only once every passed id is selected', () => {
  const { result, rerender } = renderHook(({ ids }) => useRowSelection(ids), {
    initialProps: { ids: ['a', 'b'] },
  });

  expect(result.current.allSelected).toBe(false);

  act(() => {
    result.current.toggle('a');
    result.current.toggle('b');
  });
  rerender({ ids: ['a', 'b'] });
  expect(result.current.allSelected).toBe(true);
});

test('toggleAll selects all when not all selected, and clears when all already selected', () => {
  const { result } = renderHook(() => useRowSelection(['a', 'b', 'c']));

  act(() => result.current.toggleAll(['a', 'b', 'c']));
  expect(result.current.selected).toEqual(new Set(['a', 'b', 'c']));

  act(() => result.current.toggleAll(['a', 'b', 'c']));
  expect(result.current.selected.size).toBe(0);
});

test('clear empties the selection', () => {
  const { result } = renderHook(() => useRowSelection(['a']));

  act(() => result.current.toggle('a'));
  expect(result.current.selected.size).toBe(1);

  act(() => result.current.clear());
  expect(result.current.selected.size).toBe(0);
});
