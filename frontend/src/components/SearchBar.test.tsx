import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import SearchBar from './SearchBar';

afterEach(() => {
  vi.useRealTimers();
});

test('renders the current value and a search icon', () => {
  render(<SearchBar value="garage" onChange={vi.fn()} />);
  expect(screen.getByLabelText('Search')).toHaveValue('garage');
});

test('debounces keystrokes: onChange is not called immediately, but is called once typing settles', () => {
  vi.useFakeTimers();
  const onChange = vi.fn();

  render(<SearchBar value="" onChange={onChange} />);
  fireEvent.change(screen.getByLabelText('Search'), { target: { value: 'lobby' } });

  expect(onChange).not.toHaveBeenCalled();

  act(() => {
    vi.advanceTimersByTime(400);
  });
  expect(onChange).toHaveBeenCalledWith('lobby');
});

test('re-syncs its draft when the controlled value changes externally', () => {
  const { rerender } = render(<SearchBar value="" onChange={vi.fn()} />);
  rerender(<SearchBar value="reset" onChange={vi.fn()} />);
  expect(screen.getByLabelText('Search')).toHaveValue('reset');
});
