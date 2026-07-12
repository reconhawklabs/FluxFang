// A tiny dependency-free SVG sparkline of signal strength (RSSI) over time —
// embedded in the Co-Travel Details view under the sighting-points map. Skips
// null-RSSI samples; needs at least two real points to draw a line, otherwise
// renders a small empty-state note.
export interface RssiPoint {
  observed_at: string;
  signal_strength: number | null;
}

export interface RssiSparklineProps {
  points: RssiPoint[];
  width?: number;
  height?: number;
  className?: string;
}

export default function RssiSparkline({
  points,
  width = 240,
  height = 40,
  className = '',
}: RssiSparklineProps) {
  const usable = points
    .filter((p): p is { observed_at: string; signal_strength: number } => p.signal_strength !== null)
    .slice()
    .sort((a, b) => a.observed_at.localeCompare(b.observed_at));

  if (usable.length < 2) {
    return (
      <div data-testid="rssi-sparkline-empty" className={`text-xs text-slate-500 ${className}`}>
        No signal-strength history.
      </div>
    );
  }

  const times = usable.map((p) => new Date(p.observed_at).getTime());
  const rssis = usable.map((p) => p.signal_strength);
  const tMin = Math.min(...times);
  const tMax = Math.max(...times);
  const rMin = Math.min(...rssis);
  const rMax = Math.max(...rssis);
  const tSpan = tMax - tMin || 1;
  const rSpan = rMax - rMin || 1;
  const pad = 3;

  const coords = usable.map((_, i) => {
    const x = pad + ((times[i] - tMin) / tSpan) * (width - 2 * pad);
    // Stronger signal (less-negative RSSI) plots higher (smaller y).
    const y = pad + (1 - (rssis[i] - rMin) / rSpan) * (height - 2 * pad);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });

  return (
    <svg
      data-testid="rssi-sparkline"
      width={width}
      height={height}
      className={`text-amber-400 ${className}`}
      role="img"
      aria-label="Signal strength over time"
    >
      <polyline fill="none" stroke="currentColor" strokeWidth="1.5" points={coords.join(' ')} />
    </svg>
  );
}
