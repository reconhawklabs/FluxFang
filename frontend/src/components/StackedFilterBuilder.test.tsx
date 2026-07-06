import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import StackedFilterBuilder from './StackedFilterBuilder';
import type { Condition } from '../types/rule';
import type { FieldDef } from '../types/catalog';
import { mockFetchRoutes } from '../test-utils/fetchMocks';

afterEach(() => {
  vi.unstubAllGlobals();
});

const WIFI_CATALOG: FieldDef[] = [
  {
    key: 'bssid',
    label: 'BSSID',
    type: 'mac',
    ops: [
      { code: 'eq', label: 'is exactly' },
      { code: 'in', label: 'is any of' },
    ],
  },
  {
    key: 'channel',
    label: 'Channel',
    type: 'number',
    ops: [{ code: 'gte', label: 'is at least' }],
  },
];

function renderBuilder(value: Condition[], onChange: (next: Condition[]) => void) {
  vi.stubGlobal('fetch', mockFetchRoutes({ '/api/catalog/wifi': WIFI_CATALOG }));
  const queryClient = new QueryClient();
  return render(
    <QueryClientProvider client={queryClient}>
      <StackedFilterBuilder kind="wifi" value={value} onChange={onChange} />
    </QueryClientProvider>,
  );
}

test('shows the first condition row even when value is empty, with no "Add Additional Filter" button yet', async () => {
  renderBuilder([], vi.fn());

  expect(await screen.findByTestId('condition-row-0')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /add additional filter/i })).not.toBeInTheDocument();
});

test('completing the first row (field+op+value) reveals an enabled "Add Additional Filter" button', async () => {
  renderBuilder(
    [{ field: 'bssid', op: 'eq', value: '' }],
    vi.fn(),
  );
  await screen.findByTestId('condition-row-0');

  // Incomplete (empty value) -> no add button yet.
  expect(screen.queryByRole('button', { name: /add additional filter/i })).not.toBeInTheDocument();

  // Now render with a *complete* condition and confirm the button appears, enabled.
  const onChange = vi.fn();
  renderBuilder([{ field: 'bssid', op: 'eq', value: 'aa:bb:cc:dd:ee:ff' }], onChange);
  const addButton = await screen.findByRole('button', { name: /add additional filter/i });
  expect(addButton).toBeEnabled();
});

test('clicking "Add Additional Filter" appends a new empty condition row', async () => {
  const onChange = vi.fn();
  renderBuilder([{ field: 'bssid', op: 'eq', value: 'aa:bb:cc:dd:ee:ff' }], onChange);

  const addButton = await screen.findByRole('button', { name: /add additional filter/i });
  fireEvent.click(addButton);

  expect(onChange).toHaveBeenCalledWith([
    { field: 'bssid', op: 'eq', value: 'aa:bb:cc:dd:ee:ff' },
    { field: 'bssid', op: 'eq', value: '' },
  ]);
});

test('a second, incomplete row hides the add-button again', async () => {
  renderBuilder(
    [
      { field: 'bssid', op: 'eq', value: 'aa:bb:cc:dd:ee:ff' },
      { field: 'channel', op: 'gte', value: '' },
    ],
    vi.fn(),
  );

  await screen.findByTestId('condition-row-1');
  expect(screen.queryByRole('button', { name: /add additional filter/i })).not.toBeInTheDocument();
});

test('editing the always-shown first (virtual) row turns it into value[0]', async () => {
  const onChange = vi.fn();
  renderBuilder([], onChange);

  const row = await screen.findByTestId('condition-row-0');
  const opSelect = row.querySelector('select[id$="-op"]') as HTMLSelectElement;
  fireEvent.change(opSelect, { target: { value: 'in' } });

  expect(onChange).toHaveBeenCalledWith([{ field: 'bssid', op: 'in', value: [] }]);
});
