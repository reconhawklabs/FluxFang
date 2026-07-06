import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import SelectionToolbar from './SelectionToolbar';

afterEach(() => {
  vi.restoreAllMocks();
});

test('"Delete selected" is disabled at N=0 and enabled once N>0', () => {
  const { rerender } = render(
    <SelectionToolbar
      selectedCount={0}
      onDeleteSelected={vi.fn()}
      onClearAll={vi.fn()}
      itemLabelPlural="Emissions"
    />,
  );
  expect(screen.getByRole('button', { name: /delete selected \(0\)/i })).toBeDisabled();

  rerender(
    <SelectionToolbar
      selectedCount={2}
      onDeleteSelected={vi.fn()}
      onClearAll={vi.fn()}
      itemLabelPlural="Emissions"
    />,
  );
  expect(screen.getByRole('button', { name: /delete selected \(2\)/i })).toBeEnabled();
});

test('"Delete selected" calls onDeleteSelected only after a confirmed window.confirm', () => {
  const onDeleteSelected = vi.fn();
  vi.spyOn(window, 'confirm').mockReturnValue(true);

  render(
    <SelectionToolbar
      selectedCount={3}
      onDeleteSelected={onDeleteSelected}
      onClearAll={vi.fn()}
      itemLabelPlural="Emissions"
    />,
  );
  fireEvent.click(screen.getByRole('button', { name: /delete selected \(3\)/i }));

  expect(window.confirm).toHaveBeenCalled();
  expect(onDeleteSelected).toHaveBeenCalled();
});

test('declining the confirm dialog does not call the callback', () => {
  const onDeleteSelected = vi.fn();
  vi.spyOn(window, 'confirm').mockReturnValue(false);

  render(
    <SelectionToolbar
      selectedCount={3}
      onDeleteSelected={onDeleteSelected}
      onClearAll={vi.fn()}
      itemLabelPlural="Emissions"
    />,
  );
  fireEvent.click(screen.getByRole('button', { name: /delete selected \(3\)/i }));

  expect(onDeleteSelected).not.toHaveBeenCalled();
});

test('"Clear All <label>" confirms then calls onClearAll', () => {
  const onClearAll = vi.fn();
  vi.spyOn(window, 'confirm').mockReturnValue(true);

  render(
    <SelectionToolbar selectedCount={0} onDeleteSelected={vi.fn()} onClearAll={onClearAll} itemLabelPlural="Emissions" />,
  );
  fireEvent.click(screen.getByRole('button', { name: /clear all emissions/i }));

  expect(window.confirm).toHaveBeenCalled();
  expect(onClearAll).toHaveBeenCalled();
});
