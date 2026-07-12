import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import CoTravel from './CoTravel';
import * as coTravelApi from '../api/coTravel';

vi.mock('../api/coTravel');

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <CoTravel />
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
    fireEvent.click(screen.getByRole('button', { name: /ignore/i }));
    await waitFor(() => expect(coTravelApi.ignoreEmitter).toHaveBeenCalledWith('e1'));
  });

  it('shows a "top N of total" note when results are capped', async () => {
    vi.mocked(coTravelApi.listCoTravel).mockResolvedValue({ items: [item], total: 750 });
    renderPage();
    await waitFor(() => expect(screen.getByText('wifi_client:aa:bb')).toBeInTheDocument());
    expect(screen.getByText(/showing top 1 of 750 emitters/i)).toBeInTheDocument();
  });
});
