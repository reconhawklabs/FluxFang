// The Sensors fleet page (Standalone only). Manages distributed Sensor nodes:
// pending-approval registrations, approved sensors + health, and the
// enrollment window. Consumes the Phase 3A operator endpoints.
import { useSensors } from '../hooks/useSensors';

export default function Sensors() {
  const { data: sensors = [], isLoading } = useSensors();

  const pending = sensors.filter((s) => s.status === 'pending');
  const registered = sensors.filter((s) => s.status === 'approved');

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-lg font-semibold text-slate-100">Sensors</h1>
        <p className="mt-1 text-sm text-slate-400">
          Distributed Sensor nodes reporting to this Standalone.
        </p>
      </div>

      <section data-testid="pending-section" className="space-y-2">
        <h2 className="text-sm font-medium uppercase tracking-wide text-slate-500">
          Pending approval
        </h2>
        {pending.length === 0 ? (
          <p className="text-sm text-slate-500">No sensors awaiting approval.</p>
        ) : (
          <p className="text-sm text-slate-400">{pending.length} pending</p>
        )}
      </section>

      <section data-testid="registered-section" className="space-y-2">
        <h2 className="text-sm font-medium uppercase tracking-wide text-slate-500">
          Registered sensors
        </h2>
        {isLoading ? (
          <p className="text-sm text-slate-500">Loading…</p>
        ) : registered.length === 0 ? (
          <p className="text-sm text-slate-500">No approved sensors yet.</p>
        ) : (
          <p className="text-sm text-slate-400">{registered.length} registered</p>
        )}
      </section>
    </div>
  );
}
