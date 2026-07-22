// The Dashboard shown on a Sensor node: forwarding status (cache depth,
// undelivered backlog, standalone target) + recent captured emissions.
import { useQuery } from '@tanstack/react-query';
import { useSensorStatus } from '../hooks/useSensorStatus';
import { queryKeys } from '../api/queryKeys';
import { api } from '../api/client';

export default function SensorDashboard() {
  const status = useSensorStatus();
  const cached = useQuery({ queryKey: [...queryKeys.cachedEmissions, 50], queryFn: () => api.cachedEmissions(50), refetchInterval: 4000 });
  const s = status.data;
  const rows = cached.data ?? [];

  return (
    <div className="space-y-6">
      <h1 className="text-xl font-semibold text-slate-100">Sensor</h1>
      <section data-testid="forwarding-status" className="grid grid-cols-2 gap-4 sm:grid-cols-3">
        <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-4">
          <div className="text-xs uppercase tracking-wide text-slate-500">Cached</div>
          <div className="text-2xl font-semibold text-slate-100">{s?.cache.total ?? 0}</div>
        </div>
        <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-4">
          <div className="text-xs uppercase tracking-wide text-slate-500">Undelivered</div>
          <div className={`text-2xl font-semibold ${(s?.cache.undelivered ?? 0) > 0 ? 'text-amber-400' : 'text-emerald-400'}`}>{s?.cache.undelivered ?? 0}</div>
        </div>
        <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-4">
          <div className="text-xs uppercase tracking-wide text-slate-500">Forwarding to</div>
          <div className="truncate font-mono text-sm text-slate-200">{s?.sensor ? `${s.sensor.host}:${s.sensor.port}` : '—'}</div>
        </div>
      </section>

      <section className="space-y-2">
        <h2 className="text-sm font-semibold uppercase tracking-wide text-slate-400">Recent captures</h2>
        {rows.length === 0 ? (
          <p className="text-sm text-slate-500">No captures yet.</p>
        ) : (
          <ul className="divide-y divide-slate-800 rounded border border-slate-800 text-sm">
            {rows.map((r) => (
              <li key={r.id} data-testid={`cached-${r.id}`} className="flex justify-between px-3 py-2">
                <span className="font-mono text-slate-300">{r.kind}</span>
                <span className={r.delivered ? 'text-emerald-400' : 'text-amber-400'}>{r.delivered ? 'delivered' : 'pending'}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
