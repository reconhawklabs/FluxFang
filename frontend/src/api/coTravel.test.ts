import { describe, expect, it, vi, beforeEach } from 'vitest';
import { listCoTravel, ignoreEmitter } from './coTravel';

describe('coTravel api', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('builds the co-travel query string from params', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ items: [], total: 0 }), { status: 200 }),
    );
    await listCoTravel({ min_distance_m: 402.336, min_time_s: 30, limit: 25 });
    const url = fetchMock.mock.calls[0][0] as string;
    expect(url).toContain('/api/co-travel?');
    expect(url).toContain('min_distance_m=402.336');
    expect(url).toContain('min_time_s=30');
    expect(url).toContain('limit=25');
  });

  it('POSTs to the ignore endpoint', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response('', { status: 200 }),
    );
    await ignoreEmitter('abc');
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/api/co-travel/ignore/abc');
    expect((init as RequestInit).method).toBe('POST');
  });
});
