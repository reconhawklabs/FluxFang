// Task 9.2 acceptance tests. `mockCatalog` stands in for
// `GET /api/catalog/:kind` via the shared `mockFetchRoutes` helper — these
// exercise the full `RuleBuilder` (catalog fetch + `ConditionRow` +
// match-mode toggle + preview), not just the pure helpers/`ConditionRow`
// unit tests in `conditionUtils.test.ts`/`ConditionRow.test.tsx`.
import { useState } from 'react';
import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import RuleBuilder from './RuleBuilder';
import type { FieldDef } from '../types/catalog';
import type { Rule } from '../types/rule';
import { mockFetchRoutes } from '../test-utils/fetchMocks';

afterEach(() => {
  vi.unstubAllGlobals();
});

function mockCatalog(kind: string, fields: FieldDef[], extraRoutes: Record<string, unknown> = {}) {
  const fetchMock = mockFetchRoutes({ [`/api/catalog/${kind}`]: fields, ...extraRoutes });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

function renderWithClient(ui: ReactNode) {
  const queryClient = new QueryClient();
  return render(<QueryClientProvider client={queryClient}>{ui}</QueryClientProvider>);
}

/** Controlled harness so a test can add a condition (via the component's own
 * "+ Add condition" button) and then interact with the freshly-rendered row,
 * mirroring how a real caller re-renders `RuleBuilder` after `onChange`. */
function Harness({ kind, initial, showPreview }: { kind: string; initial: Rule; showPreview?: boolean }) {
  const [rule, setRule] = useState<Rule>(initial);
  return <RuleBuilder kind={kind} value={rule} onChange={setRule} showPreview={showPreview} />;
}

const wifiCatalog: FieldDef[] = [
  {
    key: 'bssid',
    label: 'BSSID',
    type: 'mac',
    ops: [
      { code: 'eq', label: 'is exactly' },
      { code: 'matches', label: 'contains / matches' },
    ],
  },
  {
    key: 'channel',
    label: 'Channel',
    type: 'number',
    ops: [
      { code: 'eq', label: 'is exactly' },
      { code: 'gte', label: 'is at least' },
      { code: 'lte', label: 'is at most' },
    ],
  },
  {
    key: 'frame_type',
    label: 'Frame type',
    type: 'enum',
    values: ['beacon', 'probe_request'],
    ops: [{ code: 'eq', label: 'is exactly' }],
  },
];

test('operator options come from the catalog and are selectable, not typed', async () => {
  mockCatalog('wifi', wifiCatalog);

  const emptyRule: Rule = { match: 'all', conditions: [{ field: 'bssid', op: 'eq', value: '' }] };
  renderWithClient(<RuleBuilder kind="wifi" value={emptyRule} onChange={() => {}} />);

  await screen.findByText('BSSID');
  const opSelect = screen.getByLabelText(/operator/i);
  expect(within(opSelect).getByText('is exactly')).toBeInTheDocument();
  expect(within(opSelect).getByText('contains / matches')).toBeInTheDocument();
});

test('a number field\'s gte operator with 6 typed in produces a condition whose value is the number 6', async () => {
  mockCatalog('wifi', wifiCatalog);

  renderWithClient(<Harness kind="wifi" initial={{ match: 'all', conditions: [{ field: 'channel', op: 'eq', value: '' }] }} />);
  await screen.findByText('Channel');

  fireEvent.change(screen.getByLabelText(/operator/i), { target: { value: 'gte' } });
  fireEvent.change(screen.getByLabelText(/value/i), { target: { value: '6' } });

  await waitFor(() => expect(screen.getByLabelText(/operator/i)).toHaveValue('gte'));
  expect(screen.getByLabelText(/value/i)).toHaveValue(6);
});

test('changing the field resets the operator to a valid one for the new field\'s type', async () => {
  mockCatalog('wifi', wifiCatalog);

  renderWithClient(<Harness kind="wifi" initial={{ match: 'all', conditions: [{ field: 'bssid', op: 'matches', value: 'x' }] }} />);
  await screen.findByText('BSSID');

  fireEvent.change(screen.getByLabelText(/field/i), { target: { value: 'channel' } });

  await waitFor(() => expect(screen.getByLabelText(/field/i)).toHaveValue('channel'));
  // 'matches' isn't a valid op for a number field -> must have reset.
  expect(screen.getByLabelText(/operator/i)).toHaveValue('eq');
});

test('the match-mode toggle offers Match ALL / Match ANY and calls onChange', async () => {
  mockCatalog('wifi', wifiCatalog);
  const onChange = vi.fn();

  renderWithClient(
    <RuleBuilder kind="wifi" value={{ match: 'all', conditions: [] }} onChange={onChange} />,
  );
  await screen.findByRole('button', { name: /add condition/i });

  expect(screen.getByText('Match ALL')).toBeInTheDocument();
  expect(screen.getByText('Match ANY')).toBeInTheDocument();

  fireEvent.change(screen.getByDisplayValue('Match ALL'), { target: { value: 'any' } });
  expect(onChange).toHaveBeenCalledWith({ match: 'any', conditions: [] });
});

test('add condition seeds the first catalog field/op, and remove condition removes the row', async () => {
  mockCatalog('wifi', wifiCatalog);
  const onChange = vi.fn();

  renderWithClient(<RuleBuilder kind="wifi" value={{ match: 'all', conditions: [] }} onChange={onChange} />);
  const addButton = await screen.findByRole('button', { name: /add condition/i });

  fireEvent.click(addButton);
  expect(onChange).toHaveBeenCalledWith({
    match: 'all',
    conditions: [{ field: 'bssid', op: 'eq', value: '' }],
  });
});

test('showPreview fetches /api/emitters/preview (debounced) and renders the match count once a condition is complete', async () => {
  const fetchMock = mockCatalog('wifi', wifiCatalog, {
    '/api/emitters/preview': { match_count: 3 },
  });

  renderWithClient(
    <Harness
      kind="wifi"
      initial={{ match: 'all', conditions: [{ field: 'bssid', op: 'eq', value: 'aa:bb:cc:dd:ee:ff' }] }}
      showPreview
    />,
  );

  // Real timers throughout: the debounce (400ms) is short enough to just
  // wait out for real rather than fighting fake-timers/async-polling
  // interaction.
  await screen.findByText('Matches 3 emissions', {}, { timeout: 2000 });
  expect(fetchMock.mock.calls.some((call) => String(call[0]).includes('/api/emitters/preview'))).toBe(true);
});

test('showPreview does not query until a condition is complete', async () => {
  const fetchMock = mockCatalog('wifi', wifiCatalog, { '/api/emitters/preview': { match_count: 99 } });

  renderWithClient(
    <RuleBuilder kind="wifi" value={{ match: 'all', conditions: [] }} onChange={() => {}} showPreview />,
  );

  await screen.findByText(/add a condition to preview matches/i);
  expect(fetchMock.mock.calls.some((call) => String(call[0]).includes('/api/emitters/preview'))).toBe(false);
});
