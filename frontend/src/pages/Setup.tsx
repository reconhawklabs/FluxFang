import { useState } from 'react';
import type { FormEvent } from 'react';
import { api, ApiError } from '../api/client';
import type { NodeRole } from '../api/client';
import { generateKey } from '../lib/sensorKey';

const MAX_PASSWORD_LENGTH = 1024;
const SENSOR_ID_RE = /^[A-Za-z0-9_-]{1,64}$/;
const DEFAULT_TTL_SECS = 604_800; // 7 days

export interface SetupProps {
  /** Called after a successful `POST /api/setup` (which also logs the
   * caller in — see backend `auth_routes::setup`) so `App.tsx` can re-run
   * `useAuth` and move past the setup screen. */
  onSetupComplete: () => void | Promise<void>;
}

export default function Setup({ onSetupComplete }: SetupProps) {
  const [password, setPassword] = useState('');
  const [confirm, setConfirm] = useState('');
  const [role, setRole] = useState<NodeRole>('standalone');
  const [nodeId, setNodeId] = useState('local');
  const [host, setHost] = useState('');
  const [port, setPort] = useState('9000');
  const [key, setKey] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // When switching to Sensor, clear the standalone default id so the operator
  // must name the sensor; switching back restores 'local'.
  function selectRole(next: NodeRole): void {
    setRole(next);
    setNodeId(next === 'sensor' ? '' : 'local');
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    setError(null);

    if (password.length === 0 || password.length > MAX_PASSWORD_LENGTH) {
      setError('Choose a password.');
      return;
    }
    if (password !== confirm) {
      setError('Passwords do not match.');
      return;
    }
    if (!SENSOR_ID_RE.test(nodeId)) {
      setError('Use a sensor id with no spaces (letters, numbers, - or _).');
      return;
    }

    const portNum = Number(port);
    if (role === 'sensor') {
      if (host.trim().length === 0) {
        setError('Enter the standalone host.');
        return;
      }
      if (!Number.isInteger(portNum) || portNum <= 0 || portNum > 65535) {
        setError('Enter a valid port.');
        return;
      }
      if (key.length === 0) {
        setError('Generate or paste an encryption key.');
        return;
      }
    }

    setSubmitting(true);
    try {
      await api.setup(
        role === 'sensor'
          ? {
              password,
              role,
              node_sensor_id: nodeId,
              sensor: { host: host.trim(), port: portNum, key, cache_ttl_secs: DEFAULT_TTL_SECS },
            }
          : { password, role, node_sensor_id: nodeId },
      );
      await onSetupComplete();
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) {
        setError('Setup has already been completed.');
      } else {
        setError('Setup failed. Try again.');
      }
    } finally {
      setSubmitting(false);
    }
  }

  const inputClass =
    'w-full rounded border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
  const labelClass = 'block text-xs font-medium uppercase tracking-wide text-slate-500';

  return (
    <div className="flex min-h-screen items-center justify-center bg-slate-950 px-4 py-8">
      <form
        onSubmit={(event) => void handleSubmit(event)}
        className="w-full max-w-sm space-y-4 rounded-lg border border-slate-800 bg-slate-900 p-8 shadow-xl"
      >
        <div>
          <h1 className="font-mono text-lg font-semibold text-amber-400">FluxFang</h1>
          <p className="mt-1 text-sm text-slate-400">Set an admin password and choose this node&apos;s role.</p>
        </div>

        <fieldset className="space-y-2">
          <legend className={labelClass}>Node role</legend>
          <label className="flex items-center gap-2 text-sm text-slate-200">
            <input type="radio" name="role" value="standalone" checked={role === 'standalone'} onChange={() => selectRole('standalone')} />
            Standalone Node
          </label>
          <label className="flex items-center gap-2 text-sm text-slate-200">
            <input type="radio" name="role" value="sensor" checked={role === 'sensor'} onChange={() => selectRole('sensor')} />
            Sensor Node
          </label>
        </fieldset>

        <div className="space-y-1">
          <label htmlFor="setup-password" className={labelClass}>Password</label>
          <input id="setup-password" type="password" autoComplete="new-password" value={password}
            onChange={(e) => setPassword(e.target.value)} className={inputClass} />
        </div>
        <div className="space-y-1">
          <label htmlFor="setup-confirm" className={labelClass}>Confirm password</label>
          <input id="setup-confirm" type="password" autoComplete="new-password" value={confirm}
            onChange={(e) => setConfirm(e.target.value)} className={inputClass} />
        </div>

        {role === 'sensor' && (
          <>
            <div className="space-y-1">
              <label htmlFor="setup-sensor-id" className={labelClass}>Sensor id</label>
              <input id="setup-sensor-id" value={nodeId} onChange={(e) => setNodeId(e.target.value)}
                placeholder="frontgate" className={inputClass} />
            </div>
            <div className="space-y-1">
              <label htmlFor="setup-host" className={labelClass}>Standalone host</label>
              <input id="setup-host" value={host} onChange={(e) => setHost(e.target.value)}
                placeholder="base.example" className={inputClass} />
            </div>
            <div className="space-y-1">
              <label htmlFor="setup-port" className={labelClass}>Port</label>
              <input id="setup-port" inputMode="numeric" value={port} onChange={(e) => setPort(e.target.value)}
                className={inputClass} />
            </div>
            <div className="space-y-1">
              <label htmlFor="setup-key" className={labelClass}>Encryption key</label>
              <div className="flex gap-2">
                <input id="setup-key" value={key} onChange={(e) => setKey(e.target.value)} className={inputClass} />
                <button type="button" onClick={() => setKey(generateKey())}
                  className="shrink-0 rounded border border-slate-700 px-3 py-2 text-sm text-slate-300 hover:border-slate-500 hover:text-slate-100">
                  Generate
                </button>
              </div>
            </div>
          </>
        )}

        {error && <p role="alert" className="text-sm text-red-400">{error}</p>}

        <button type="submit" disabled={submitting}
          className="w-full rounded bg-amber-500 px-3 py-2 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50">
          {submitting ? 'Setting up…' : 'Finish setup'}
        </button>
      </form>
    </div>
  );
}
