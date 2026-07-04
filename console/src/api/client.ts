// Typed fetch clients for the two backends the console talks to: the WSF trust
// plane (`wsf-api`) and the AOG gateway (`aog-gateway`). This is the TypeScript
// SDK the W6 DEVLOG deferred to Phase C. No dependencies — just `fetch`.

import type {
  AttenuateReq,
  ExchangeReq,
  ExchangeResp,
  IssueReq,
  ReceiptsResp,
  TokenResp,
  TrustToken,
  UsageResp,
  VerifyResp,
} from './types';

/** An HTTP error carrying the status and the server's message body. */
export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

function trimBase(base: string): string {
  return base.replace(/\/+$/, '');
}

async function jsonFetch<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, init);
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new ApiError(res.status, body || res.statusText || `HTTP ${res.status}`);
  }
  return (await res.json()) as T;
}

function jsonBody(value: unknown): RequestInit {
  return {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(value),
  };
}

/** Client for the WSF trust-plane REST surface (`wsf-api`). */
export class WsfClient {
  private readonly base: string;

  constructor(base: string) {
    this.base = trimBase(base);
  }

  /** Liveness — `GET /healthz`. Never throws; returns reachability. */
  async health(): Promise<boolean> {
    try {
      const res = await fetch(`${this.base}/healthz`);
      return res.ok;
    } catch {
      return false;
    }
  }

  /** Verify a trust token's signature + revocation + expiry. */
  verifyToken(token: TrustToken): Promise<VerifyResp> {
    return jsonFetch<VerifyResp>(`${this.base}/v1/tokens/verify`, jsonBody({ token }));
  }

  /** Issue a new trust token for a tenant/subject. */
  issueToken(req: IssueReq): Promise<TokenResp> {
    return jsonFetch<TokenResp>(`${this.base}/v1/tokens/issue`, jsonBody(req));
  }

  /** Mint a narrower child token (must narrow the parent on every axis). */
  attenuate(req: AttenuateReq): Promise<TokenResp> {
    return jsonFetch<TokenResp>(`${this.base}/v1/tokens/attenuate`, jsonBody(req));
  }

  /** Exchange a verified token for ephemeral, scoped cloud credentials. */
  exchange(req: ExchangeReq): Promise<ExchangeResp> {
    return jsonFetch<ExchangeResp>(`${this.base}/v1/credentials/exchange`, jsonBody(req));
  }

  /** Query the unified receipt ledger. With no field/value, returns all. */
  receipts(field?: string, value?: string): Promise<ReceiptsResp> {
    const qs =
      field && value
        ? `?field=${encodeURIComponent(field)}&value=${encodeURIComponent(value)}`
        : '';
    return jsonFetch<ReceiptsResp>(`${this.base}/v1/receipts${qs}`);
  }

  /** The published OpenAPI document. */
  openapi(): Promise<unknown> {
    return jsonFetch<unknown>(`${this.base}/openapi.json`);
  }
}

/** Client for the AOG gateway. Endpoints that meter/govern require the caller's
 *  virtual key as a bearer (the same key an OpenAI/Anthropic SDK would send). */
export class AogClient {
  private readonly base: string;
  private readonly bearer?: string;

  constructor(base: string, bearer?: string) {
    this.base = trimBase(base);
    this.bearer = bearer;
  }

  private authHeaders(): Record<string, string> {
    return this.bearer ? { authorization: `Bearer ${this.bearer}` } : {};
  }

  /** Liveness — `GET /healthz`. Never throws; returns reachability. */
  async health(): Promise<boolean> {
    try {
      const res = await fetch(`${this.base}/healthz`);
      return res.ok;
    } catch {
      return false;
    }
  }

  /** Metering aggregates + live receipt-chain integrity (`GET /v1/usage`). */
  usage(): Promise<UsageResp> {
    return jsonFetch<UsageResp>(`${this.base}/v1/usage`, { headers: this.authHeaders() });
  }
}
