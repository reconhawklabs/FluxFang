// Co-Travel Detection API client. Mirrors `fluxfang-api::cotravel`'s DTOs.
import { del, get, post } from './client';

export type CoTravelTier = 'critical' | 'high' | 'medium' | 'low' | 'minimal';

/** One ranked row — mirrors the backend `CoTravelDto`. */
export interface CoTravelItem {
  emitter_id: string;
  name: string;
  emitter_type: string | null;
  identity_key: string | null;
  attributes: Record<string, unknown>;
  hits: number;
  points: number;
  span_s: number;
  spread_m: number;
  first_seen: string;
  last_seen: string;
  score: number;
  tier: CoTravelTier;
}

export interface CoTravelPage {
  items: CoTravelItem[];
  total: number;
}

/** One ignored emitter — mirrors the backend `IgnoredEmitter`. */
export interface IgnoredEmitter {
  id: string;
  name: string;
  emitter_type: string | null;
  identity_key: string | null;
  attributes: Record<string, unknown>;
}

export interface CoTravelParams {
  from?: string;
  to?: string;
  min_distance_m?: number;
  min_time_s?: number;
  limit?: number;
  offset?: number;
}

export function listCoTravel(params: CoTravelParams = {}): Promise<CoTravelPage> {
  const q = new URLSearchParams();
  if (params.from !== undefined) q.set('from', params.from);
  if (params.to !== undefined) q.set('to', params.to);
  if (params.min_distance_m !== undefined) q.set('min_distance_m', String(params.min_distance_m));
  if (params.min_time_s !== undefined) q.set('min_time_s', String(params.min_time_s));
  if (params.limit !== undefined) q.set('limit', String(params.limit));
  if (params.offset !== undefined) q.set('offset', String(params.offset));
  const qs = q.toString();
  return get<CoTravelPage>(`/api/co-travel${qs.length > 0 ? `?${qs}` : ''}`);
}

export function ignoreEmitter(emitterId: string): Promise<void> {
  return post<void>(`/api/co-travel/ignore/${encodeURIComponent(emitterId)}`);
}

export function unignoreEmitter(emitterId: string): Promise<{ removed: number }> {
  return del<{ removed: number }>(`/api/co-travel/ignore/${encodeURIComponent(emitterId)}`);
}

export function listIgnored(): Promise<IgnoredEmitter[]> {
  return get<IgnoredEmitter[]>('/api/co-travel/ignored');
}
