// Typed fetch clients for the two backends the console talks to: the WSF trust
// plane (`wsf-api`) and the AOG gateway (`aog-gateway`). This is the TypeScript
// SDK the W6 DEVLOG deferred to Phase C. No dependencies — just `fetch`.

import type {
  ApprovalActionResp,
  ApprovalsResp,
  AttenuateReq,
  DenialExplanation,
  ExchangeReq,
  ExchangeResp,
  IssueReq,
  PolicyMode,
  PolicyResp,
  PolicyRule,
  ReceiptsResp,
  SessionResp,
  SessionsResp,
  StatusResp,
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

  private post<T>(path: string, body: unknown): Promise<T> {
    return jsonFetch<T>(`${this.base}${path}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', ...this.authHeaders() },
      body: JSON.stringify(body),
    });
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

  /** The pending human-approval inbox (T3) — side-effecting tool calls awaiting a
   *  decision, each with a diff preview. */
  approvals(): Promise<ApprovalsResp> {
    return jsonFetch<ApprovalsResp>(`${this.base}/v1/approvals`, {
      headers: this.authHeaders(),
    });
  }

  /** Approve a pending request; records the actor. */
  approve(id: string, actor: string): Promise<ApprovalActionResp> {
    return this.post<ApprovalActionResp>(
      `/v1/approvals/${encodeURIComponent(id)}/approve`,
      { actor },
    );
  }

  /** Deny a pending request with a reason; records the actor. */
  deny(id: string, actor: string, reason: string): Promise<ApprovalActionResp> {
    return this.post<ApprovalActionResp>(`/v1/approvals/${encodeURIComponent(id)}/deny`, {
      actor,
      reason,
    });
  }

  /** The policy-as-code rule set (G6), for the Policy Studio. */
  policy(): Promise<PolicyResp> {
    return jsonFetch<PolicyResp>(`${this.base}/v1/policy`, { headers: this.authHeaders() });
  }

  /** Set a rule's enforcement mode (shadow → report_only → enforce). */
  setPolicyMode(ruleId: string, mode: PolicyMode): Promise<PolicyRule> {
    return this.post<PolicyRule>(`/v1/policy/${encodeURIComponent(ruleId)}/mode`, { mode });
  }

  /** Explain a specific denial in plain language, citing the exact policy line. */
  explainDenial(decision: string): Promise<DenialExplanation> {
    return jsonFetch<DenialExplanation>(
      `${this.base}/v1/policy/explain?decision=${encodeURIComponent(decision)}`,
      { headers: this.authHeaders() },
    );
  }

  /** The list of recorded agent sessions (T7). */
  sessions(): Promise<SessionsResp> {
    return jsonFetch<SessionsResp>(`${this.base}/v1/sessions`, {
      headers: this.authHeaders(),
    });
  }

  /** One session's full, deterministically-replayable transcript. */
  session(id: string): Promise<SessionResp> {
    return jsonFetch<SessionResp>(`${this.base}/v1/sessions/${encodeURIComponent(id)}`, {
      headers: this.authHeaders(),
    });
  }

  /** Metering aggregates + live receipt-chain integrity (`GET /v1/usage`). */
  usage(): Promise<UsageResp> {
    return jsonFetch<UsageResp>(`${this.base}/v1/usage`, { headers: this.authHeaders() });
  }

  /** The gateway's live posture: mode, providers, models, receipt-chain integrity
   *  (`GET /v1/status`, open — no virtual key required). */
  status(): Promise<StatusResp> {
    return jsonFetch<StatusResp>(`${this.base}/v1/status`);
  }
}
