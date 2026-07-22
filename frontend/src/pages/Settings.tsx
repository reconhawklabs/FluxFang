// Edit this node's config post-setup (PATCH /api/config). Both roles. The
// sensor encryption key is write-only (blank = keep current, since the API
// never returns it). Switching role requires a backend restart to take effect.
import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useConfig } from '../hooks/useConfig';
import { api } from '../api/client';
import type { ConfigPatch, NodeRole } from '../api/client';

const SLUG_RE = /^[A-Za-z0-9_-]{1,64}$/;

export default function Settings() {
  const { data: config } = useConfig();
  const queryClient = useQueryClient();

  const [nodeId, setNodeId] = useState('');
  const [role, setRole] = useState<NodeRole>('standalone');
  const [host, setHost] = useState('');
  const [port, setPort] = useState('9000');
  const [ttl, setTtl] = useState('604800');
  const [key, setKey] = useState(''); // write-only; blank = keep
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!config) return;
    setNodeId(config.node_sensor_id);
    setRole(config.role);
    if (config.sensor) {
      setHost(config.sensor.host);
      setPort(String(config.sensor.port));
      setTtl(String(config.sensor.cache_ttl_secs));
    }
  }, [config]);

  const mutation = useMutation({
    mutationFn: (patch: ConfigPatch) => api.updateConfig(patch),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['config'] }),
  });

  function handleSubmit(e: FormEvent<HTMLFormElement>): void {
    e.preventDefault();
    setError(null);
    if (!SLUG_RE.test(nodeId)) { setError('Node id: letters, numbers, - or _ (no spaces).'); return; }
    const patch: ConfigPatch = { node_sensor_id: nodeId, role };
    if (role === 'sensor') {
      const portNum = Number(port);
      if (host.trim() === '' || !Number.isInteger(portNum) || portNum < 1 || portNum > 65535) {
        setError('Enter a valid standalone host and port.'); return;
      }
      if (!config?.sensor && key.trim() === '') {
        setError('An encryption key is required when switching this node to Sensor role.');
        return;
      }
      patch.sensor = { host: host.trim(), port: portNum, cache_ttl_secs: Number(ttl) || 604800 };
      if (key.length > 0) patch.sensor.key = key; // only send if changed
    }
    mutation.mutate(patch);
  }

  const input = 'w-full rounded border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
  const label = 'block text-xs font-medium uppercase tracking-wide text-slate-500';

  return (
    <div className="max-w-lg space-y-6">
      <h1 className="text-lg font-semibold text-slate-100">Settings</h1>
      <form onSubmit={handleSubmit} className="space-y-4 rounded-lg border border-slate-800 bg-slate-900 p-6">
        <div className="space-y-1">
          <label htmlFor="s-node-id" className={label}>Node sensor id</label>
          <input id="s-node-id" value={nodeId} onChange={(e) => setNodeId(e.target.value)} className={input} />
        </div>

        <fieldset className="space-y-2">
          <legend className={label}>Node role</legend>
          <label className="flex items-center gap-2 text-sm text-slate-200">
            <input type="radio" name="role" checked={role === 'standalone'} onChange={() => setRole('standalone')} /> Standalone
          </label>
          <label className="flex items-center gap-2 text-sm text-slate-200">
            <input type="radio" name="role" checked={role === 'sensor'} onChange={() => setRole('sensor')} /> Sensor
          </label>
          {config && role !== config.role && (
            <p className="text-xs text-amber-400">Changing role takes effect after a backend restart.</p>
          )}
        </fieldset>

        {role === 'sensor' && (
          <>
            <div className="space-y-1"><label htmlFor="s-host" className={label}>Standalone host</label>
              <input id="s-host" value={host} onChange={(e) => setHost(e.target.value)} className={input} /></div>
            <div className="space-y-1"><label htmlFor="s-port" className={label}>Port</label>
              <input id="s-port" inputMode="numeric" value={port} onChange={(e) => setPort(e.target.value)} className={input} /></div>
            <div className="space-y-1"><label htmlFor="s-ttl" className={label}>Cache TTL (seconds)</label>
              <input id="s-ttl" inputMode="numeric" value={ttl} onChange={(e) => setTtl(e.target.value)} className={input} /></div>
            <div className="space-y-1"><label htmlFor="s-key" className={label}>Encryption key</label>
              <input id="s-key" value={key} onChange={(e) => setKey(e.target.value)} placeholder="•••••• (unchanged)" className={input} />
              <p className="text-xs text-slate-500">{config?.sensor ? 'Leave blank to keep the current key.' : 'Required when switching to Sensor role.'}</p></div>
          </>
        )}

        {error && <p role="alert" className="text-sm text-red-400">{error}</p>}
        {mutation.isError && <p role="alert" className="text-sm text-red-400">Save failed.</p>}
        {mutation.isSuccess && <p className="text-sm text-emerald-400">Saved.</p>}

        <button type="submit" disabled={mutation.isPending}
          className="rounded bg-amber-500 px-3 py-2 text-sm font-semibold text-slate-950 hover:bg-amber-400 disabled:opacity-50">
          {mutation.isPending ? 'Saving…' : 'Save'}
        </button>
      </form>
    </div>
  );
}
