// Covers the plain-text-body fix to `errorMessage()`: several endpoints
// (emissions/emitters `400`s — see `fluxfang-api::emissions`/`emitters`'s
// `ApiError::into_response`) send their validation message as a raw text
// body, not JSON. Before the fix, `errorMessage` only tried `res.json()` and
// fell straight back to `statusText` on any parse failure, so these specific
// messages never reached the UI.
import { afterEach, expect, test, vi } from 'vitest';
import { ApiError, get } from './client';

afterEach(() => {
  vi.unstubAllGlobals();
});

function mockResponse(opts: { status: number; statusText: string; text: string; json?: () => unknown }): Response {
  return {
    ok: opts.status >= 200 && opts.status < 300,
    status: opts.status,
    statusText: opts.statusText,
    text: async () => opts.text,
    json: async () => {
      if (opts.json) return opts.json();
      throw new Error('not json');
    },
    clone() {
      return this;
    },
  } as unknown as Response;
}

test('a 400 with a plain-text body surfaces that text as the ApiError message', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(
      mockResponse({
        status: 400,
        statusText: 'Bad Request',
        text: 'invalid cond "bssid:eq": malformed cond (expected field:op:value)',
      }),
    ),
  );

  await expect(get('/api/emissions')).rejects.toMatchObject({
    message: 'invalid cond "bssid:eq": malformed cond (expected field:op:value)',
    status: 400,
  });
});

test('a 400 with a JSON {message} body still takes priority over any text body', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(
      mockResponse({
        status: 400,
        statusText: 'Bad Request',
        text: '{"message":"structured message"}',
        json: () => ({ message: 'structured message' }),
      }),
    ),
  );

  await expect(get('/api/emissions')).rejects.toMatchObject({ message: 'structured message' });
});

test('a response with no body at all falls back to statusText', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(mockResponse({ status: 401, statusText: 'Unauthorized', text: '' })));

  await expect(get('/api/whatever')).rejects.toMatchObject({ message: 'Unauthorized' });
  await expect(get('/api/whatever')).rejects.toBeInstanceOf(ApiError);
});
