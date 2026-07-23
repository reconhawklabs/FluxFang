// The Dashboard shown on a Sensor node: forwarding status (cache depth,
// undelivered backlog, standalone target) + recent captured emissions.
import { useQuery } from '@tanstack/react-query';
import { useSensorStatus } from '../hooks/useSensorStatus';
import type { ForwarderStatus } from '../api/client';
import { queryKeys } from '../api/queryKeys';
import { api } from '../api/client';
import {
  formatObservedAt,
  payloadRecord,
  payloadTextAny,
} from '../lib/emissionPayload';

/** Recent-captures feed cap — a dashboard glance, not the full Emissions page. */
const FEED_LIMIT = 10;

export default function SensorDashboard() {
  const status = useSensorStatus();
  const cached = useQuery({ queryKey: [...queryKeys.cachedEmissions, FEED_LIMIT], queryFn: () => api.cachedEmissions(FEED_LIMIT), refetchInterval: 4000 });
  const s = status.data;
  const rows = cached.data ?? [];
  const forwarding = describeForwarding(s?.forwarding);

  return (
    <div className="space-y-6">
      <h1 className="text-xl font-semibold text-slate-100">Sensor</h1>
      <section data-testid="forwarding-status" className="grid grid-cols-2 gap-4 sm:grid-cols-3">
        <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-4">
          <div className="text-xs uppercase tracking-wide text-slate-500">Standalone</div>
          {s?.connected == null ? (
            <div className="text-2xl font-semibold text-slate-500">—</div>
          ) : (
            <div className={`flex items-center gap-2 text-2xl font-semibold ${s.connected ? 'text-emerald-400' : 'text-red-400'}`}>
              <span className={`inline-block h-2.5 w-2.5 rounded-full ${s.connected ? 'bg-emerald-400' : 'bg-red-400'}`} aria-hidden="true" />
              {s.connected ? 'Reachable' : 'Offline'}
            </div>
          )}
        </div>
        <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-4">
          <div className="text-xs uppercase tracking-wide text-slate-500">Forwarding</div>
          <div
            data-testid="forwarding-state"
            className={`flex items-center gap-2 text-2xl font-semibold ${forwarding.tone}`}
          >
            <span
              className={`inline-block h-2.5 w-2.5 rounded-full ${forwarding.dot}`}
              aria-hidden="true"
            />
            {forwarding.label}
          </div>
        </div>
        <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-4">
          <div className="text-xs uppercase tracking-wide text-slate-500">Delivered (1h)</div>
          <div className="text-2xl font-semibold text-slate-100">{s?.delivered_last_hour ?? 0}</div>
        </div>
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

      {s?.forwarding?.last_error && (
        <p
          role="alert"
          data-testid="forwarding-error"
          className="rounded border border-amber-900/60 bg-amber-950/30 px-3 py-2 text-sm text-amber-300"
        >
          Forwarding problem: {s.forwarding.last_error}
        </p>
      )}

      <section className="space-y-2">
        <h2 className="text-sm font-semibold uppercase tracking-wide text-slate-400">Recent captures</h2>
        {rows.length === 0 ? (
          <p className="text-sm text-slate-500">No captures yet.</p>
        ) : (
          <div className="overflow-x-auto rounded border border-slate-800">
            <table className="w-full border-collapse text-left text-sm">
              <thead>
                <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
                  <th className="px-3 py-2 font-medium">Kind</th>
                  <th className="px-3 py-2 font-medium">Identity</th>
                  <th className="px-3 py-2 font-medium">SSID/Name</th>
                  <th className="px-3 py-2 font-medium">RSSI</th>
                  <th className="px-3 py-2 font-medium">Seen</th>
                  <th className="px-3 py-2 font-medium">Status</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((r) => {
                  const payload = payloadRecord(r.payload);
                  return (
                    <tr key={r.id} data-testid={`cached-${r.id}`} className="border-b border-slate-900 last:border-0">
                      <td className="px-3 py-2 font-mono text-slate-300">{r.kind}</td>
                      <td className="px-3 py-2 font-mono text-slate-300">
                        {payloadTextAny(payload, ['bssid', 'src_mac', 'address'])}
                      </td>
                      <td className="px-3 py-2 text-slate-300">
                        {payloadTextAny(payload, ['ssid', 'name'])}
                      </td>
                      <td className="px-3 py-2 font-mono text-slate-300">{r.signal_strength ?? '—'}</td>
                      <td className="px-3 py-2 text-slate-400">{formatObservedAt(r.observed_at)}</td>
                      <td className={`px-3 py-2 ${r.delivered ? 'text-emerald-400' : 'text-amber-400'}`}>
                        {r.delivered ? 'delivered' : 'pending'}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}

/** How to render the forwarding tile.
 *
 * The Standalone tile above is a reachability probe of its listener, which is
 * why a sensor could show "Connected" while the Standalone listed it as down:
 * the listener was up the whole time and every batch was still failing. This
 * reads the forwarding loop's own outcome, so the two tiles disagreeing is
 * itself the diagnosis rather than a contradiction. */
function describeForwarding(f: ForwarderStatus | undefined): {
  label: string;
  tone: string;
  dot: string;
} {
  if (!f) return { label: '—', tone: 'text-slate-500', dot: 'bg-slate-600' };
  // An error outranks the state: "Forwarding" with every batch failing is the
  // misleading reading this whole tile exists to prevent.
  if (f.last_error) return { label: 'Failing', tone: 'text-red-400', dot: 'bg-red-400' };
  switch (f.state) {
    case 'forwarding':
      return { label: 'Delivering', tone: 'text-emerald-400', dot: 'bg-emerald-400' };
    case 'enrolling':
      return { label: 'Awaiting approval', tone: 'text-amber-400', dot: 'bg-amber-400' };
    case 'paused':
      return { label: 'Not configured', tone: 'text-slate-400', dot: 'bg-slate-600' };
  }
}
