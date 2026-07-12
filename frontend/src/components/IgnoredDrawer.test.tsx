import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import IgnoredDrawer from './IgnoredDrawer';
import * as coTravelApi from '../api/coTravel';

vi.mock('../api/coTravel');

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{ui}</QueryClientProvider>;
}

describe('IgnoredDrawer', () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(coTravelApi.listIgnored).mockResolvedValue([
      { id: 'e1', name: 'BT', emitter_type: 'wifi_client', identity_key: 'wifi_client:aa:bb', attributes: {} },
    ]);
    vi.mocked(coTravelApi.unignoreEmitter).mockResolvedValue({ removed: 1 });
  });

  it('lists ignored emitters and restores one', async () => {
    render(wrap(<IgnoredDrawer open onClose={() => {}} />));
    await waitFor(() => expect(screen.getByText('wifi_client:aa:bb')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /restore/i }));
    await waitFor(() => expect(coTravelApi.unignoreEmitter).toHaveBeenCalledWith('e1'));
  });

  it('renders nothing when closed', () => {
    render(wrap(<IgnoredDrawer open={false} onClose={() => {}} />));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});
