import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import FilterBar from './FilterBar';
import { EMPTY_FILTER_STATE, filterToQueryParams } from './filterState';
import type { FilterState } from './filterState';
import type { FieldDef } from '../types/catalog';
import { mockFetchRoutes } from '../test-utils/fetchMocks';

afterEach(() => {
  vi.unstubAllGlobals();
});

const wifiCatalog: FieldDef[] = [
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

function renderWithClient(kind: string, value: FilterState, onChange: (next: FilterState) => void) {
  const queryClient = new QueryClient();
  return render(
    <QueryClientProvider client={queryClient}>
      <FilterBar kind={kind} value={value} onChange={onChange} />
    </QueryClientProvider>,
  );
}

test('renders a search box and calls onChange with the typed text', () => {
  vi.stubGlobal('fetch', mockFetchRoutes({ '/api/catalog/wifi': wifiCatalog }));
  const onChange = vi.fn();

  renderWithClient('wifi', EMPTY_FILTER_STATE, onChange);

  fireEvent.change(screen.getByLabelText(/search/i), { target: { value: 'garage' } });
  expect(onChange).toHaveBeenCalledWith({ ...EMPTY_FILTER_STATE, q: 'garage' });
});

test('adding a filter condition reuses the same field/operator dropdown machinery as RuleBuilder', async () => {
  vi.stubGlobal('fetch', mockFetchRoutes({ '/api/catalog/wifi': wifiCatalog }));
  const onChange = vi.fn();

  renderWithClient('wifi', EMPTY_FILTER_STATE, onChange);

  fireEvent.click(await screen.findByRole('button', { name: /add filter/i }));
  expect(onChange).toHaveBeenCalledWith({
    ...EMPTY_FILTER_STATE,
    conditions: [{ field: 'bssid', op: 'eq', value: '' }],
  });
});

test('the unassigned-only checkbox toggles FilterState.unassigned', () => {
  vi.stubGlobal('fetch', mockFetchRoutes({ '/api/catalog/wifi': wifiCatalog }));
  const onChange = vi.fn();

  renderWithClient('wifi', EMPTY_FILTER_STATE, onChange);

  fireEvent.click(screen.getByLabelText(/unassigned only/i));
  expect(onChange).toHaveBeenCalledWith({ ...EMPTY_FILTER_STATE, unassigned: true });
});

test('filterToQueryParams: q + complete conditions become query params; incomplete conditions are dropped', () => {
  const state: FilterState = {
    q: '  living room  ',
    unassigned: true,
    conditions: [
      { field: 'bssid', op: 'eq', value: 'aa:bb:cc:dd:ee:ff' },
      { field: 'channel', op: 'gte', value: 6 },
      { field: 'bssid', op: 'in', value: ['aa:aa', 'bb:bb'] },
      { field: '', op: '', value: '' }, // incomplete, never chosen a field
    ],
  };

  const params = filterToQueryParams(state);

  expect(params.get('q')).toBe('living room');
  expect(params.get('unassigned')).toBe('true');
  expect(params.getAll('cond')).toEqual([
    'bssid:eq:"aa:bb:cc:dd:ee:ff"',
    'channel:gte:6',
    'bssid:in:["aa:aa","bb:bb"]',
  ]);
});

test('filterToQueryParams quotes a numeric-looking string value (Text/mac/enum field) but emits a number bare', () => {
  const state: FilterState = {
    q: '',
    conditions: [
      // A Text/enum-typed field (e.g. `ssid`) whose value happens to look
      // like a JSON number/bool/null: must be JSON-quoted so the backend's
      // JSON-first `parse_condition` parses it back as a STRING, not a
      // number/bool — this is the regression the fix targets.
      { field: 'ssid', op: 'eq', value: '2024' },
      { field: 'ssid', op: 'eq', value: 'true' },
      { field: 'ssid', op: 'eq', value: 'false' },
      { field: 'ssid', op: 'eq', value: 'null' },
      // A genuine `number`-typed field value must stay bare so it parses
      // back as a JSON number.
      { field: 'channel', op: 'gte', value: 6 },
    ],
  };

  const params = filterToQueryParams(state);

  expect(params.getAll('cond')).toEqual([
    'ssid:eq:"2024"',
    'ssid:eq:"true"',
    'ssid:eq:"false"',
    'ssid:eq:"null"',
    'channel:gte:6',
  ]);
});

test('filterToQueryParams omits q/unassigned when empty/false and produces no conditions for an empty state', () => {
  const params = filterToQueryParams(EMPTY_FILTER_STATE);
  expect(params.get('q')).toBeNull();
  expect(params.get('unassigned')).toBeNull();
  expect(params.getAll('cond')).toEqual([]);
});
