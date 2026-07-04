import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AogClient, WsfClient } from '../api/client';
import type { TrustToken } from '../api/types';

function ok(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
  } as unknown as Response;
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('WsfClient', () => {
  it('verifyToken POSTs to /v1/tokens/verify and returns the verdict', async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve(ok({ valid: true, reason: 'ok' })),
    );
    vi.stubGlobal('fetch', fetchMock);

    const verdict = await new WsfClient('http://x').verifyToken(
      { token_id: 't' } as unknown as TrustToken,
    );
    expect(verdict).toEqual({ valid: true, reason: 'ok' });

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('http://x/v1/tokens/verify');
    expect(init?.method).toBe('POST');
  });

  it('receipts builds a correlation query and trims a trailing slash', async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve(ok({ entries: [] })),
    );
    vi.stubGlobal('fetch', fetchMock);

    await new WsfClient('http://x/').receipts('token_id', 'tok_1');
    expect(fetchMock.mock.calls[0][0]).toBe(
      'http://x/v1/receipts?field=token_id&value=tok_1',
    );
  });

  it('receipts with no args hits the unfiltered endpoint', async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve(ok({ entries: [] })),
    );
    vi.stubGlobal('fetch', fetchMock);

    await new WsfClient('http://x').receipts();
    expect(fetchMock.mock.calls[0][0]).toBe('http://x/v1/receipts');
  });

  it('throws ApiError with the status on a non-2xx response', async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve({
        ok: false,
        status: 403,
        json: async () => ({}),
        text: async () => 'forbidden',
      } as unknown as Response),
    );
    vi.stubGlobal('fetch', fetchMock);

    await expect(
      new WsfClient('http://x').verifyToken({ token_id: 't' } as unknown as TrustToken),
    ).rejects.toMatchObject({ status: 403 });
  });
});

describe('AogClient', () => {
  it('usage sends the virtual key as a bearer', async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve(ok({ tasks: [], chain_head: '00', chain_verified: true })),
    );
    vi.stubGlobal('fetch', fetchMock);

    await new AogClient('http://g', 'vk_123').usage();
    const init = fetchMock.mock.calls[0][1];
    const headers = (init?.headers ?? {}) as Record<string, string>;
    expect(headers.authorization).toBe('Bearer vk_123');
  });

  it('usage omits the auth header when no key is set', async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve(ok({ tasks: [], chain_head: '00', chain_verified: true })),
    );
    vi.stubGlobal('fetch', fetchMock);

    await new AogClient('http://g').usage();
    const init = fetchMock.mock.calls[0][1];
    const headers = (init?.headers ?? {}) as Record<string, string>;
    expect(headers.authorization).toBeUndefined();
  });
});
