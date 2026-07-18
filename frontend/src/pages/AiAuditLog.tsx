// AI Audit Log — Task 15 UI for the embedded MCP server's audit trail
// (Task 14 backend, `GET /api/ai-audit`). Lists every AI-made add/remove
// action with an action filter + prev/next pagination.
import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { queryKeys } from '../api/queryKeys';
import { listAiAudit, type ListAiAuditParams } from '../api/aiAudit';

const PAGE_SIZE = 50;

export default function AiAuditLog() {
  const [action, setAction] = useState<'' | 'add' | 'remove'>('');
  const [offset, setOffset] = useState(0);

  const params: ListAiAuditParams = {
    limit: PAGE_SIZE,
    offset,
    ...(action ? { action } : {}),
  };
  const query = useQuery({
    queryKey: [...queryKeys.aiAudit, params],
    queryFn: () => listAiAudit(params),
  });

  const items = query.data?.items ?? [];
  const total = query.data?.total ?? 0;
  const hasPrev = offset > 0;
  const hasNext = offset + items.length < total;

  return (
    <div className="space-y-4 p-4">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold text-slate-100">AI Audit Log</h1>
        <select
          className="rounded border border-slate-700 bg-slate-900 px-2 py-1 text-sm text-slate-200"
          value={action}
          onChange={(e) => { setAction(e.target.value as '' | 'add' | 'remove'); setOffset(0); }}
          data-testid="audit-action-filter"
        >
          <option value="">All actions</option>
          <option value="add">Additions</option>
          <option value="remove">Subtractions</option>
        </select>
      </div>

      {query.isLoading && <p className="text-sm text-slate-500">Loading audit log…</p>}
      {query.isError && <p className="text-sm text-red-400">Failed to load audit log.</p>}
      {query.data && items.length === 0 && (
        <p className="text-sm text-slate-500">No AI actions recorded yet.</p>
      )}

      {items.length > 0 && (
        <table className="w-full text-left text-sm">
          <thead className="text-slate-400">
            <tr>
              <th className="py-1 pr-4">When</th>
              <th className="py-1 pr-4">Action</th>
              <th className="py-1 pr-4">Tool</th>
              <th className="py-1 pr-4">Summary</th>
              <th className="py-1 pr-4">Status</th>
            </tr>
          </thead>
          <tbody>
            {items.map((row) => (
              <tr key={row.id} data-testid={`audit-row-${row.id}`} className="border-t border-slate-800">
                <td className="py-1 pr-4 text-slate-400">{new Date(row.created_at).toLocaleString()}</td>
                <td className="py-1 pr-4">
                  <span
                    data-testid={`audit-action-${row.id}`}
                    className={`rounded px-2 py-0.5 text-xs ${
                      row.action === 'add' ? 'bg-emerald-500/10 text-emerald-400' : 'bg-red-500/10 text-red-400'
                    }`}
                  >
                    {row.action}
                  </span>
                </td>
                <td className="py-1 pr-4 font-mono text-slate-200">{row.tool}</td>
                <td className="py-1 pr-4 text-slate-300">{row.summary}</td>
                <td className="py-1 pr-4">
                  <span className={row.status === 'ok' ? 'text-slate-400' : 'text-red-400'}>{row.status}</span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <div className="flex items-center gap-3 text-sm text-slate-400">
        <span>{total === 0 ? 0 : offset + 1}–{Math.min(offset + items.length, total)} of {total}</span>
        <button disabled={!hasPrev} onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
          className="rounded border border-slate-700 px-2 py-1 disabled:opacity-40">Previous</button>
        <button disabled={!hasNext} onClick={() => setOffset(offset + PAGE_SIZE)}
          className="rounded border border-slate-700 px-2 py-1 disabled:opacity-40">Next</button>
      </div>
    </div>
  );
}
