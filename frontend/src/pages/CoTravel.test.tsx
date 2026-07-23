import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import CoTravel from './CoTravel';
import * as coTravelApi from '../api/coTravel';

vi.mock('../api/coTravel');

// CoTravelRow -> CoTravelDetails -> SightingPointsMap transitively imports
// maplibre-gl. Rows aren't expanded in these tests so no map instantiates,
// but this keeps the import inert (same fake used in SightingPointsMap.test.tsx).
vi.mock('maplibre-gl', () => ({
  default: {
    Map: class {
      constructor() {}
      addControl() {}
      on(_e: string, cb: () => void) {
        if (_e === 'load') cb();
      }
      remove() {}
      addSource() {}
      addLayer() {}
      getSource() {
        return { setData() {} };
      }
      fitBounds() {}
    },
    NavigationControl: class {},
  },
}));

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // Rows link to /emitters/:id, so a router must be in scope -- in the app
  // this page is itself a route, so this matches how it actually renders.
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <CoTravel />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const item = {
  emitter_id: 'e1',
  name: 'BT Client',
  emitter_type: 'wifi_client',
  identity_key: 'wifi_client:aa:bb',
  attributes: {},
  hits: 15,
  points: 7,
  span_s: 1734,
  spread_m: 60000,
  first_seen: '2026-07-11T14:02:00Z',
  last_seen: '2026-07-11T14:35:00Z',
  score: 69,
  tier: 'high' as const,
};

describe('CoTravel page', () => {
  beforeEach(() => {
    vi.mocked(coTravelApi.listCoTravel).mockResolvedValue({ items: [item], total: 1 });
    vi.mocked(coTravelApi.listIgnored).mockResolvedValue([]);
    vi.mocked(coTravelApi.ignoreEmitter).mockResolvedValue(undefined);
  });

  it('renders a tier group and the emitter row', async () => {
    renderPage();
    await waitFor(() => expect(screen.getByText('wifi_client:aa:bb')).toBeInTheDocument());
    expect(screen.getByText(/HIGH/i)).toBeInTheDocument();
  });

  it('calls ignoreEmitter when Ignore is clicked', async () => {
    renderPage();
    await waitFor(() => expect(screen.getByText('wifi_client:aa:bb')).toBeInTheDocument());
    // Exact match: the header's new "Ignored (N)" link also matches /ignore/i.
    fireEvent.click(screen.getByRole('button', { name: 'Ignore' }));
    await waitFor(() => expect(coTravelApi.ignoreEmitter).toHaveBeenCalledWith('e1'));
  });

  it('shows a "top N of total" note when results are capped', async () => {
    vi.mocked(coTravelApi.listCoTravel).mockResolvedValue({ items: [item], total: 750 });
    renderPage();
    await waitFor(() => expect(screen.getByText('wifi_client:aa:bb')).toBeInTheDocument());
    expect(screen.getByText(/showing top 1 of 750 emitters/i)).toBeInTheDocument();
  });

  it('sends from/to (RFC3339) to listCoTravel when the date window is set', async () => {
    renderPage();
    await waitFor(() => expect(screen.getByText('wifi_client:aa:bb')).toBeInTheDocument());
    const fromInput = screen.getByLabelText(/from/i);
    fireEvent.change(fromInput, { target: { value: '2026-07-11T10:00' } });
    await waitFor(() => {
      // Timezone-robust: a `datetime-local` -> `toISOString()` conversion
      // shifts local->UTC, so the calendar date can roll over depending on
      // the test runner's timezone. Assert an RFC3339 `from` was sent at
      // all, not a hard-coded calendar date.
      const calledWithFrom = vi
        .mocked(coTravelApi.listCoTravel)
        .mock.calls.some(([p]) => typeof p?.from === 'string' && /^\d{4}-\d{2}-\d{2}T/.test(p.from));
      expect(calledWithFrom).toBe(true);
    });
  });

  it('opens the ignored drawer from the header link', async () => {
    vi.mocked(coTravelApi.listIgnored).mockResolvedValue([
      { id: 'e9', name: 'X', emitter_type: 'wifi_client', identity_key: 'wifi_client:zz', attributes: {} },
    ]);
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: /ignored/i }));
    await waitFor(() => expect(screen.getByRole('dialog')).toBeInTheDocument());
    expect(screen.getByText('wifi_client:zz')).toBeInTheDocument();
  });
});
