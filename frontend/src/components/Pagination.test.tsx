import { fireEvent, render, screen } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import Pagination from './Pagination';

test('renders the "N-M of TOTAL" label and page-size options', () => {
  render(<Pagination total={120} limit={50} offset={50} onChange={vi.fn()} />);
  expect(screen.getByText('51–100 of 120')).toBeInTheDocument();
  expect(screen.getByLabelText(/page size/i)).toHaveValue('50');
});

test('Prev/Next are disabled at the bounds', () => {
  const { rerender } = render(<Pagination total={10} limit={50} offset={0} onChange={vi.fn()} />);
  expect(screen.getByRole('button', { name: /prev/i })).toBeDisabled();
  expect(screen.getByRole('button', { name: /next/i })).toBeDisabled();

  rerender(<Pagination total={120} limit={50} offset={50} onChange={vi.fn()} />);
  expect(screen.getByRole('button', { name: /prev/i })).toBeEnabled();
  expect(screen.getByRole('button', { name: /next/i })).toBeEnabled();
});

test('Next advances offset by limit; Prev retreats, clamped at 0', () => {
  const onChange = vi.fn();
  render(<Pagination total={120} limit={50} offset={50} onChange={onChange} />);

  fireEvent.click(screen.getByRole('button', { name: /next/i }));
  expect(onChange).toHaveBeenCalledWith(50, 100);

  fireEvent.click(screen.getByRole('button', { name: /prev/i }));
  expect(onChange).toHaveBeenCalledWith(50, 0);
});

test('changing page size resets offset to 0', () => {
  const onChange = vi.fn();
  render(<Pagination total={120} limit={50} offset={50} onChange={onChange} />);

  fireEvent.change(screen.getByLabelText(/page size/i), { target: { value: '100' } });
  expect(onChange).toHaveBeenCalledWith(100, 0);
});
