import { render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import CoTravelDetails from './CoTravelDetails';
import * as emissionsApi from '../api/emissions';

vi.mock('../api/emissions');
// SightingPointsMap pulls in maplibre-gl; mock it to a stub so this test
// stays focused on the fetch+compose behavior.
vi.mock('./SightingPointsMap', () => ({
  default: ({ points }: { points: unknown[] }) => (
    <div data-testid="mock-map">points:{points.length}</div>
  ),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{ui}</QueryClientProvider>;
}

const emission = (lon: number | null, lat: number | null, rssi: number | null, t: string) => ({
  id: t,
  data_source_id: null,
  emitter_id: 'e1',
  session_id: null,
  observed_at: t,
  signal_strength: rssi,
  lon,
  lat,
  kind: 'wifi',
  payload: {},
});

describe('CoTravelDetails', () => {
  beforeEach(() => vi.resetAllMocks());

  it('renders the map (located points only) and the sparkline', async () => {
    vi.mocked(emissionsApi.listEmissions).mockResolvedValue({
      items: [
        emission(-84.5, 37.7, -70, '2026-07-11T14:00:00Z'),
        emission(-84.4, 37.6, -60, '2026-07-11T14:05:00Z'),
        emission(null, null, -50, '2026-07-11T14:07:00Z'), // unlocated: excluded from map
      ],
      total: 3,
    });
    render(wrap(<CoTravelDetails emitterId="e1" />));
    await waitFor(() => expect(screen.getByTestId('mock-map')).toHaveTextContent('points:2'));
    expect(screen.getByTestId('rssi-sparkline')).toBeInTheDocument();
  });

  it('passes emitter_id and window into the emissions query', async () => {
    vi.mocked(emissionsApi.listEmissions).mockResolvedValue({ items: [], total: 0 });
    render(wrap(<CoTravelDetails emitterId="e1" from="2026-07-11T10:00:00.000Z" to="2026-07-11T16:00:00.000Z" />));
    await waitFor(() => expect(emissionsApi.listEmissions).toHaveBeenCalled());
    const params = vi.mocked(emissionsApi.listEmissions).mock.calls[0][0];
    expect(params.get('emitter_id')).toBe('e1');
    expect(params.get('time_from')).toBe('2026-07-11T10:00:00.000Z');
    expect(params.get('time_to')).toBe('2026-07-11T16:00:00.000Z');
  });
});
