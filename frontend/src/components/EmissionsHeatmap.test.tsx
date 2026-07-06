// Task C TDD target: `EmissionsHeatmap` is the reusable compact heatmap
// embedded in `Emitters.tsx`'s and `Entities.tsx`'s detail panels. Per the
// task brief, GL rendering itself isn't under test (that's what
// `mapData.test.ts`'s `pointsToHeatmapGeoJSON` covers) — this file only
// checks the component's non-GL surface: it renders a map container when
// given points, and an empty-state message when given none.
// `maplibre-gl` is mocked wholesale (same convention as `MapView.test.tsx`)
// so `new maplibregl.Map(...)` never touches a real WebGL canvas.
import { render, screen } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import EmissionsHeatmap from './EmissionsHeatmap';

vi.mock('maplibre-gl', () => {
  class FakeMap {
    private handlers = new Map<string, () => void>();
    constructor(_options: unknown) {}
    addControl(): void {}
    on(event: string, cb: () => void): void {
      this.handlers.set(event, cb);
      if (event === 'load') cb();
    }
    remove(): void {}
    resize(): void {}
    addSource(): void {}
    addLayer(): void {}
    getSource() {
      return { setData: vi.fn() };
    }
    getLayer() {
      return true;
    }
    setLayoutProperty(): void {}
    fitBounds(): void {}
  }

  class FakeNavigationControl {}

  return { default: { Map: FakeMap, NavigationControl: FakeNavigationControl } };
});

test('renders the map container when given located points', () => {
  render(<EmissionsHeatmap points={[{ lon: 2.5, lat: 1.5 }, { lon: -1.1, lat: 5.5 }]} />);

  expect(screen.getByTestId('emissions-heatmap-container')).toBeInTheDocument();
  expect(screen.queryByTestId('emissions-heatmap-empty')).not.toBeInTheDocument();
});

test('shows an empty state when there are no points', () => {
  render(<EmissionsHeatmap points={[]} />);

  expect(screen.getByText('No located detections yet.')).toBeInTheDocument();
  expect(screen.queryByTestId('emissions-heatmap-container')).not.toBeInTheDocument();
});
