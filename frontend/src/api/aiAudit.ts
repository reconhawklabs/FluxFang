// `GET /api/ai-audit` (Task 14 backend, `fluxfang-api::ai_audit`) — the
// embedded MCP server's audit trail of AI-made add/remove actions. This
// task's (15) AI Audit Log page is the only consumer.
import { get } from './client';

/** Mirrors `fluxfang-api::ai_audit::AiAuditEntryDto`. */
export interface AiAuditEntry {
  id: string;
  created_at: string;
  tool: string;
  action: 'add' | 'remove';
  summary: string;
  args: Record<string, unknown>;
  result: unknown | null;
  affected_ids: string[];
  status: 'ok' | 'error';
  error: string | null;
}

/** `GET /api/ai-audit` query params. All optional. */
export interface ListAiAuditParams {
  action?: 'add' | 'remove';
  search?: string;
  limit?: number;
  offset?: number;
}

/** `GET /api/ai-audit` response. */
export interface AiAuditPage {
  items: AiAuditEntry[];
  total: number;
}

export function listAiAudit(params: ListAiAuditParams = {}): Promise<AiAuditPage> {
  const query = new URLSearchParams();
  if (params.action) query.set('action', params.action);
  if (params.search) query.set('search', params.search);
  if (params.limit !== undefined) query.set('limit', String(params.limit));
  if (params.offset !== undefined) query.set('offset', String(params.offset));
  const qs = query.toString();
  return get<AiAuditPage>(`/api/ai-audit${qs.length > 0 ? `?${qs}` : ''}`);
}
