import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import RssiSparkline from './RssiSparkline';

describe('RssiSparkline', () => {
  it('renders a polyline when there are 2+ RSSI points', () => {
    render(
      <RssiSparkline
        points={[
          { observed_at: '2026-07-11T14:00:00Z', signal_strength: -80 },
          { observed_at: '2026-07-11T14:05:00Z', signal_strength: -60 },
          { observed_at: '2026-07-11T14:10:00Z', signal_strength: -70 },
        ]}
      />,
    );
    const svg = screen.getByTestId('rssi-sparkline');
    expect(svg).toBeInTheDocument();
    expect(svg.querySelector('polyline')).not.toBeNull();
  });

  it('shows an empty state when fewer than 2 non-null points', () => {
    render(
      <RssiSparkline
        points={[
          { observed_at: '2026-07-11T14:00:00Z', signal_strength: null },
          { observed_at: '2026-07-11T14:05:00Z', signal_strength: -60 },
        ]}
      />,
    );
    expect(screen.getByTestId('rssi-sparkline-empty')).toBeInTheDocument();
  });
});
