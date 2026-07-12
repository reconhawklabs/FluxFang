import { render, screen } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import SightingPointsMap from './SightingPointsMap';

vi.mock('maplibre-gl', () => {
  class FakeMap {
    constructor(_o: unknown) {}
    addControl(): void {}
    on(event: string, cb: () => void): void {
      if (event === 'load') cb();
    }
    remove(): void {}
    addSource(): void {}
    addLayer(): void {}
    getSource() {
      return { setData: vi.fn() };
    }
    fitBounds(): void {}
  }
  class FakeNavigationControl {}
  return { default: { Map: FakeMap, NavigationControl: FakeNavigationControl } };
});

test('renders the map container when given located points', () => {
  render(
    <SightingPointsMap
      points={[
        { lon: 2.5, lat: 1.5, signal_strength: -70 },
        { lon: -1.1, lat: 5.5, signal_strength: -50 },
      ]}
    />,
  );
  expect(screen.getByTestId('sighting-points-container')).toBeInTheDocument();
});

test('shows an empty state when there are no points', () => {
  render(<SightingPointsMap points={[]} />);
  expect(screen.getByTestId('sighting-points-empty')).toBeInTheDocument();
  expect(screen.queryByTestId('sighting-points-container')).not.toBeInTheDocument();
});
