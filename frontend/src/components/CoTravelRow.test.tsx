import { render, screen, fireEvent } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import CoTravelRow from './CoTravelRow';
import type { CoTravelItem } from '../api/coTravel';

// Stub the details so this test stays focused on row toggle + ignore wiring.
vi.mock('./CoTravelDetails', () => ({
  default: ({ emitterId }: { emitterId: string }) => (
    <div data-testid="mock-details">details:{emitterId}</div>
  ),
}));

const item: CoTravelItem = {
  emitter_id: 'e1',
  name: 'BT',
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
  tier: 'high',
};

describe('CoTravelRow', () => {
  it('renders identity and toggles details on Details click', () => {
    render(<CoTravelRow item={item} onIgnore={() => {}} />);
    expect(screen.getByText('wifi_client:aa:bb')).toBeInTheDocument();
    expect(screen.queryByTestId('mock-details')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /details/i }));
    expect(screen.getByTestId('mock-details')).toHaveTextContent('details:e1');
  });

  it('calls onIgnore with the emitter id', () => {
    const onIgnore = vi.fn();
    render(<CoTravelRow item={item} onIgnore={onIgnore} />);
    fireEvent.click(screen.getByRole('button', { name: /ignore/i }));
    expect(onIgnore).toHaveBeenCalledWith('e1');
  });
});
