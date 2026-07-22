// The Sensors fleet page (Standalone only). Manages distributed Sensor nodes:
// pending-approval registrations, approved sensors + health, and the
// enrollment window. Consumes the Phase 3A operator endpoints.
import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useSensors } from '../hooks/useSensors';
import { queryKeys } from '../api/queryKeys';
import { approveSensor, rejectSensor, revokeSensor, rotateSensor, allowSensors } from '../api/sensors';
import type { Sensor, RotatedKey } from '../api/sensors';
import { listDataSources } from '../api/dataSources';
import { ApiError } from '../api/client';
import { isValidKey } from '../lib/sensorKey';

export default function Sensors() {
  const { data: sensors = [], isLoading } = useSensors();

  const pending = sensors.filter((s) => s.status === 'pending');
  const registered = sensors.filter((s) => s.status === 'approved');

  const queryClient = useQueryClient();
  const invalidate = () => void queryClient.invalidateQueries({ queryKey: queryKeys.sensors });
  const [approving, setApproving] = useState<Sensor | null>(null);

  const rejectMutation = useMutation({ mutationFn: rejectSensor, onSuccess: invalidate });

  const { data: dataSources = [] } = useQuery({ queryKey: queryKeys.dataSources, queryFn: listDataSources });
  const sensorListener = dataSources.find((d) => d.kind === 'sensor' && d.status === 'running');
  const [rotatedKey, setRotatedKey] = useState<RotatedKey | null>(null);

  const revokeMutation = useMutation({ mutationFn: revokeSensor, onSuccess: invalidate });
  const rotateMutation = useMutation({ mutationFn: rotateSensor, onSuccess: (k) => { setRotatedKey(k); invalidate(); } });

  // Enrollment window: `allow-sensors` returns how many seconds the window
  // stays open; we track its absolute expiry and tick a live countdown so the
  // operator sees it close and the button re-enables when it hits 0.
  const [windowExpiresAt, setWindowExpiresAt] = useState<number | null>(null);
  const [remainingSecs, setRemainingSecs] = useState(0);
  const allowMutation = useMutation({
    mutationFn: () => allowSensors(sensorListener!.id),
    onSuccess: (data) => {
      setWindowExpiresAt(Date.now() + data.remaining_secs * 1000);
      invalidate();
    },
  });

  useEffect(() => {
    // Cleared (expired or approval closed the window) — hide the countdown.
    if (windowExpiresAt === null) {
      setRemainingSecs(0);
      return;
    }
    function tick(): void {
      const secs = Math.max(0, Math.ceil((windowExpiresAt! - Date.now()) / 1000));
      setRemainingSecs(secs);
      if (secs === 0) setWindowExpiresAt(null);
    }
    tick();
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, [windowExpiresAt]);

  const windowOpen = remainingSecs > 0;

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-lg font-semibold text-slate-100">Sensors</h1>
          <p className="mt-1 text-sm text-slate-400">
            Distributed Sensor nodes reporting to this Standalone.
          </p>
        </div>
        <button
          type="button"
          disabled={!sensorListener || allowMutation.isPending || windowOpen}
          onClick={() => allowMutation.mutate()}
          title={sensorListener ? undefined : 'Enable a Sensor datasource first'}
          className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
        >
          Allow new Sensors
        </button>
      </div>
      {(rejectMutation.isError || revokeMutation.isError || rotateMutation.isError || allowMutation.isError) && (
        <p role="alert" className="text-sm text-red-400">Action failed. Please retry.</p>
      )}
      {windowOpen && (
        <p className="text-sm text-emerald-400">
          Sensor enrollment window open for {remainingSecs}s
        </p>
      )}

      <section data-testid="pending-section" className="space-y-2">
        <h2 className="text-sm font-medium uppercase tracking-wide text-slate-500">
          Pending approval
        </h2>
        {pending.length === 0 ? (
          <p className="text-sm text-slate-500">No sensors awaiting approval.</p>
        ) : (
          <ul className="divide-y divide-slate-800 rounded border border-slate-800">
            {pending.map((s) => (
              <li key={s.id} data-testid={`pending-${s.sensor_id}`} className="flex items-center justify-between px-3 py-2 text-sm">
                <div>
                  <span className="font-mono text-slate-200">{s.sensor_id}</span>
                  <span className="ml-2 text-slate-500">from {s.source_ip ?? 'unknown'}</span>
                  <span className="ml-2 font-mono text-xs text-amber-400/80">fp {s.fingerprint}</span>
                </div>
                <div className="flex gap-2">
                  <button type="button" onClick={() => setApproving(s)}
                    className="rounded bg-amber-500 px-2 py-1 text-xs font-semibold text-slate-950 hover:bg-amber-400">
                    Approve
                  </button>
                  <button type="button" onClick={() => rejectMutation.mutate(s.id)}
                    className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 hover:border-slate-500">
                    Reject
                  </button>
                </div>
              </li>
            ))}
          </ul>
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
          <ul className="divide-y divide-slate-800 rounded border border-slate-800">
            {registered.map((s) => (
              <li key={s.id} className="flex items-center justify-between px-3 py-2 text-sm">
                <div className="flex items-center gap-3">
                  <span
                    data-testid={s.online ? `sensor-online-${s.sensor_id}` : `sensor-offline-${s.sensor_id}`}
                    className={`inline-block h-2 w-2 rounded-full ${s.online ? 'bg-emerald-400' : 'bg-slate-600'}`}
                  />
                  <span className="font-mono text-slate-200">{s.sensor_id}</span>
                  <span className="font-mono text-xs text-amber-400/80">fp {s.fingerprint}</span>
                  <span className="text-xs text-slate-500">{s.emissions_24h} emissions/24h</span>
                </div>
                <div className="flex gap-2">
                  <button type="button" onClick={() => rotateMutation.mutate(s.id)}
                    className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 hover:border-slate-500">
                    Rotate key
                  </button>
                  <button type="button" onClick={() => { if (window.confirm(`Revoke ${s.sensor_id}? The sensor must re-enroll to reconnect.`)) revokeMutation.mutate(s.id); }}
                    className="rounded border border-red-900/60 px-2 py-1 text-xs text-red-400 hover:border-red-700">
                    Revoke
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      {approving && (
        <ApproveDialog
          sensor={approving}
          onClose={() => setApproving(null)}
          onApproved={() => {
            setApproving(null);
            // Approving closes the enrollment window server-side, so clear the
            // client-side countdown/message too (it's local timer state).
            setWindowExpiresAt(null);
            invalidate();
          }}
        />
      )}

      {rotatedKey && (
        <div className="fixed inset-0 z-10 flex items-center justify-center bg-slate-950/70 px-4">
          <div className="w-full max-w-md space-y-4 rounded-lg border border-slate-800 bg-slate-900 p-6">
            <h3 className="text-sm font-semibold text-slate-100">New encryption key</h3>
            <p className="text-sm text-amber-400">Copy this now — it is shown once. Re-provision the sensor with it.</p>
            <code className="block break-all rounded bg-slate-950 px-3 py-2 font-mono text-sm text-slate-200">
              {rotatedKey.key}
            </code>
            <p className="font-mono text-xs text-slate-500">fingerprint {rotatedKey.fingerprint}</p>
            <div className="flex justify-end">
              <button type="button" onClick={() => setRotatedKey(null)}
                className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 hover:bg-amber-400">
                Done
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function ApproveDialog({ sensor, onClose, onApproved }: {
  sensor: Sensor;
  onClose: () => void;
  onApproved: () => void;
}) {
  const [autoGroup, setAutoGroup] = useState(true);
  const [key, setKey] = useState('');
  const mutation = useMutation({
    mutationFn: () => approveSensor(sensor.id, autoGroup, key),
    onSuccess: onApproved,
  });

  // A valid key is 32 bytes base64. Give the operator that feedback up front
  // rather than after a round-trip; the backend still verifies the fingerprint.
  const keyShapeValid = isValidKey(key);
  const showShapeHint = key.length > 0 && !keyShapeValid;

  // The backend returns 400 when the typed key doesn't reproduce the sensor's
  // fingerprint; any other failure is a generic error.
  const wrongKey = mutation.error instanceof ApiError && mutation.error.status === 400;
  const errorText = mutation.isError
    ? wrongKey
      ? "That key doesn't match this sensor's fingerprint — re-check it."
      : 'Approve failed.'
    : null;

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-slate-950/70 px-4"
      role="dialog" aria-label={`Approve ${sensor.sensor_id}`}>
      <div className="w-full max-w-sm space-y-4 rounded-lg border border-slate-800 bg-slate-900 p-6">
        <h3 className="text-sm font-semibold text-slate-100">Approve {sensor.sensor_id}</h3>
        <p className="text-sm text-slate-400">
          Confirm this fingerprint matches the one shown on the sensor node before approving:
        </p>
        <p className="rounded bg-slate-950 px-3 py-2 text-center font-mono text-lg tracking-wider text-amber-400">
          {sensor.fingerprint}
        </p>
        <div className="space-y-1">
          <label htmlFor="approve-key" className="block text-xs font-medium uppercase tracking-wide text-slate-500">
            Sensor encryption key
          </label>
          <input id="approve-key" value={key} onChange={(e) => setKey(e.target.value)}
            className="w-full rounded border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 focus:border-amber-500 focus:outline-none" />
          <p className="text-xs text-slate-500">
            Read this off the sensor node and type it here. The sensor never transmits its key; approval verifies your entry against the fingerprint above.
          </p>
          {showShapeHint && (
            <p role="alert" className="text-xs text-red-400">That doesn't look like a valid key (expected 32 bytes, base64).</p>
          )}
        </div>
        <label className="flex items-center gap-2 text-sm text-slate-200">
          <input type="checkbox" checked={autoGroup} onChange={(e) => setAutoGroup(e.target.checked)} />
          Group emissions into emitters (using existing rules)
        </label>
        {errorText && <p role="alert" className="text-sm text-red-400">{errorText}</p>}
        <div className="flex justify-end gap-2">
          <button type="button" onClick={onClose}
            className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 hover:border-slate-500">
            Cancel
          </button>
          <button type="button" disabled={mutation.isPending || !keyShapeValid} onClick={() => mutation.mutate()}
            className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 hover:bg-amber-400 disabled:opacity-50">
            Confirm
          </button>
        </div>
      </div>
    </div>
  );
}
